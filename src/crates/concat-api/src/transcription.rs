// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! On-device transcription jobs and their project-local results.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use concat_project::model::Project;
use concat_speech::transcribe::{LANGUAGES, ModelStatus, TimedSegment, TranscribeRequest};
use concat_speech::{JobStage, Transcriber};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Api, ApiError, ErrorCode, Event, EventSink, JobStatus, Started};

pub(crate) enum Target {
    Media(String),
    Clip(String),
}

impl Target {
    pub(crate) fn new(media: Option<String>, clip: Option<String>) -> Result<Self, ApiError> {
        let target = match (media, clip) {
            (Some(id), None) => Self::Media(id),
            (None, Some(id)) => Self::Clip(id),
            _ => {
                return Err(ApiError::invalid(
                    "supply exactly one of mediaId and clipId",
                ));
            }
        };
        let (_, id) = target.parts();
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(ApiError::invalid("invalid mediaId or clipId"));
        }
        Ok(target)
    }

    fn parts(&self) -> (&str, &str) {
        match self {
            Self::Media(id) => ("media", id),
            Self::Clip(id) => ("clip", id),
        }
    }

    fn output(&self, project: &Path) -> PathBuf {
        let (kind, id) = self.parts();
        project
            .join("transcripts")
            .join(format!("{kind}-{id}.json"))
    }

    fn source(&self, project: &Project) -> Result<Source, ApiError> {
        let clip = match self {
            Self::Media(_) => None,
            Self::Clip(id) => Some(
                project
                    .timelines
                    .iter()
                    .flat_map(|tl| tl.clips.iter())
                    .find(|clip| clip.id == *id)
                    .ok_or_else(|| {
                        ApiError::new(ErrorCode::NotFound, "clip is not in the project")
                    })?,
            ),
        };
        let media_id = clip.map_or(self.parts().1, |clip| &clip.media_id);
        let media = project.media_by_id(media_id).ok_or_else(|| {
            ApiError::new(ErrorCode::NotFound, "source media is not in the project")
        })?;
        if !media.has_audio || media.placeholder {
            return Err(ApiError::invalid("source media has no audio"));
        }
        let start = clip.map_or(0.0, |clip| clip.source_start);
        let window = clip
            .map(|clip| clip.duration * clip.speed)
            .or(media.duration)
            .ok_or_else(|| ApiError::invalid("source media duration is unknown"))?;
        if start < 0.0
            || !start.is_finite()
            || window <= 0.0
            || !window.is_finite()
            || !(start + window).is_finite()
        {
            return Err(ApiError::invalid("source media has no valid time range"));
        }
        let path = fs::canonicalize(&media.path)
            .ok()
            .filter(|path| path.is_file())
            .and_then(|path| path.into_os_string().into_string().ok())
            .ok_or_else(|| {
                ApiError::new(ErrorCode::NotFound, "source must be a local media file")
            })?;
        Ok(Source {
            path,
            media_id: media_id.to_owned(),
            start,
            window,
            audio_stream: clip.and_then(|clip| clip.audio_stream),
        })
    }
}

struct Source {
    path: String,
    media_id: String,
    start: f64,
    window: f64,
    audio_stream: Option<u32>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Transcript {
    media_id: String,
    clip_id: Option<String>,
    audio_stream: Option<u32>,
    model_id: String,
    source_start: f64,
    source_end: f64,
    segments: Vec<TimedSegment>,
}

impl Transcript {
    fn filter(
        &mut self,
        start: Option<f64>,
        end: Option<f64>,
        keyword: Option<&str>,
    ) -> Result<(), ApiError> {
        if [start, end]
            .into_iter()
            .flatten()
            .any(|n| !n.is_finite() || n < 0.0)
            || start.zip(end).is_some_and(|(a, b)| a >= b)
        {
            return Err(ApiError::invalid(
                "use nonnegative source seconds with start less than end",
            ));
        }
        let keyword = keyword
            .map(str::trim)
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase);
        let overlaps =
            |a: f64, b: f64| end.is_none_or(|end| a < end) && start.is_none_or(|start| b > start);
        self.segments.retain_mut(|segment| {
            if !overlaps(segment.start, segment.end)
                || keyword
                    .as_ref()
                    .is_some_and(|word| !segment.text.to_lowercase().contains(word))
            {
                return false;
            }
            segment.words.retain(|word| overlaps(word.start, word.end));
            true
        });
        Ok(())
    }

    fn save(&self, output: &Path, cancel: &AtomicBool) -> Result<(), ApiError> {
        let dir = output.parent().expect("transcript has project folder");
        let project = dir.parent().expect("transcript folder has a project");
        if project.canonicalize().map_err(io_failed)? != project {
            return Err(ApiError::new(
                ErrorCode::Refused,
                "project path changed during transcription",
            ));
        }
        match fs::create_dir(dir) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(io_failed(error)),
        }
        if fs::symlink_metadata(dir)
            .map_err(io_failed)?
            .file_type()
            .is_symlink()
        {
            return Err(ApiError::new(
                ErrorCode::Refused,
                "project transcript folder cannot be a symlink",
            ));
        }
        let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(io_failed)?;
        // serde errors on writes describe the failure, without dumping the transcript.
        serde_json::to_writer(temp.as_file_mut(), self).map_err(io_failed)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
        temp.persist(output)
            .map_err(|error| io_failed(error.error))?;
        Ok(())
    }
}

fn io_failed(error: impl std::fmt::Display) -> ApiError {
    ApiError::failed(format!("transcript file: {error}"))
}

fn cancelled() -> ApiError {
    ApiError::new(ErrorCode::Cancelled, "transcription cancelled")
}

fn choose_model<'a>(
    models: &'a [ModelStatus],
    language: Option<&str>,
    requested: Option<&str>,
) -> Result<&'a ModelStatus, ApiError> {
    let compatible =
        |model: &&ModelStatus| !model.english_only || language.is_none_or(|code| code == "en");
    let model = if let Some(id) = requested {
        let model = models
            .iter()
            .find(|model| model.id == id)
            .ok_or_else(|| ApiError::invalid("unknown Whisper modelId; see ai_status"))?;
        if !compatible(&model) {
            return Err(ApiError::invalid(
                "the requested model understands only English",
            ));
        }
        model
    } else {
        models
            .iter()
            .filter(compatible)
            .find(|model| model.downloaded && (language == Some("en") || !model.english_only))
            .or_else(|| {
                models
                    .iter()
                    .filter(compatible)
                    .find(|model| model.downloaded)
            })
            .or_else(|| {
                models.iter().find(|model| {
                    model.id
                        == if language == Some("en") {
                            "tiny.en"
                        } else {
                            "tiny"
                        }
                })
            })
            .expect("built-in Whisper model list")
    };
    if !model.downloaded {
        return Err(ApiError::new(
            ErrorCode::ModelMissing,
            format!(
                "Whisper model {} is not installed (approximately {} bytes). Download it in ElonCut Settings > Transcriber, then retry.",
                model.id, model.size_bytes
            ),
        ));
    }
    Ok(model)
}

impl Api {
    pub(crate) fn ai_status(&self) -> Value {
        let status = Transcriber::status_read_only(&self.dirs);
        json!({
            "transcription": {
                "status": "available",
                "installed": status.models.iter().any(|model| model.downloaded),
                "busy": self.transcriber.is_busy() || self.transcriber.is_downloading(),
                "transcribing": self.transcriber.is_busy(),
                "downloading": self.transcriber.is_downloading(),
                "modelsDir": status.models_dir,
                "models": status.models,
            },
            "speechGeneration": self.speech_status(),
            "vocalSeparation": { "status": "notConnected" },
            "videoEnhancement": { "status": "notConnected" },
        })
    }

    pub(crate) fn transcribe(
        &mut self,
        path: &str,
        target: Target,
        language: Option<String>,
        model_id: Option<&str>,
    ) -> Result<Started, ApiError> {
        let session = self.session(path)?;
        let language = language
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("auto"))
            .map(|asked| {
                LANGUAGES
                    .iter()
                    .find(|(code, _)| code.eq_ignore_ascii_case(asked))
                    .map(|(code, _)| (*code).to_owned())
                    .ok_or_else(|| ApiError::invalid("unknown Whisper language code"))
            })
            .transpose()?;
        let status = Transcriber::status_read_only(&self.dirs);
        let model_id = choose_model(&status.models, language.as_deref(), model_id)?
            .id
            .clone();
        let source = target.source(session.project())?;
        let path = session.path().to_owned();
        let project = Path::new(&path).canonicalize().map_err(io_failed)?;
        let output = target.output(&project);
        let output_str = output
            .to_str()
            .ok_or_else(|| ApiError::invalid("transcript path is not UTF-8"))?;
        self.writable(output_str)?;
        let clip_id = match target {
            Target::Clip(id) => Some(id),
            Target::Media(_) => None,
        };
        let request = TranscribeRequest {
            path: source.path,
            audio_stream: source.audio_stream,
            source_start: source.start,
            window: source.window,
            model_id: model_id.clone(),
            language,
        };
        let mut transcript = Transcript {
            media_id: source.media_id,
            clip_id,
            model_id,
            audio_stream: source.audio_stream,
            source_start: source.start,
            source_end: source.start + source.window,
            segments: Vec::new(),
        };
        let dirs = self.dirs.clone();
        let run = self
            .transcriber
            .begin_timed()
            .map_err(|error| ApiError::new(ErrorCode::Busy, error))?;
        let cancel = run.cancel_handle();
        self.start_transcript_job(
            path,
            output_str.to_owned(),
            cancel,
            move |progress, cancel| {
                transcript.segments =
                    run.transcribe(&dirs, &request, progress).map_err(|error| {
                        if cancel.load(Ordering::Relaxed) {
                            cancelled()
                        } else {
                            ApiError::failed(error)
                        }
                    })?;
                for segment in &mut transcript.segments {
                    segment.start += source.start;
                    segment.end += source.start;
                    for word in &mut segment.words {
                        word.start += source.start;
                        word.end += source.start;
                    }
                }
                transcript.save(&output, cancel)
            },
        )
    }

    fn start_transcript_job(
        &mut self,
        path: String,
        output: String,
        cancel: Arc<AtomicBool>,
        work: impl FnOnce(Box<dyn FnMut(JobStage) + Send>, &AtomicBool) -> Result<(), ApiError>
        + Send
        + 'static,
    ) -> Result<Started, ApiError> {
        let started = Started {
            job: self.jobs.mint(),
            path,
            output,
        };
        let id = started.job.clone();
        self.book.start(
            JobStatus::running(&id, &started.path, &started.output),
            Arc::clone(&cancel),
        );
        let events = Arc::clone(&self.events);
        let job = started.clone();
        let thread = std::thread::Builder::new()
            .name(format!("transcript {id}"))
            .spawn(move || {
                let progress = reporter(Arc::clone(&events), &job);
                let result = if cancel.load(Ordering::Relaxed) {
                    Err(cancelled())
                } else {
                    work(progress, &cancel)
                };
                events(match result {
                    Ok(()) => Event::TranscriptDone {
                        job: job.job,
                        path: job.path,
                        output: job.output,
                    },
                    Err(error) => Event::TranscriptFailed {
                        job: job.job,
                        path: job.path,
                        error,
                    },
                });
            })
            .map_err(|error| {
                self.book.abandon(&id);
                ApiError::failed(format!("could not start the transcription: {error}"))
            })?;
        self.jobs.transcripts.insert(id.clone());
        self.jobs.threads.push((id, thread));
        Ok(started)
    }

    pub(crate) fn query_transcript(
        &self,
        path: &str,
        target: &Target,
        start: Option<f64>,
        end: Option<f64>,
        keyword: Option<&str>,
    ) -> Result<Value, ApiError> {
        let session = self.session(path)?;
        let project = Path::new(session.path())
            .canonicalize()
            .map_err(io_failed)?;
        let file = target.output(&project);
        let missing = || {
            ApiError::new(
                ErrorCode::NotFound,
                "no saved transcript for this target; run media_transcribe first",
            )
        };
        let canonical = file.canonicalize().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                missing()
            } else {
                io_failed(error)
            }
        })?;
        if canonical != file || !canonical.is_file() {
            return Err(ApiError::new(
                ErrorCode::Refused,
                "transcript must be a regular file within the project",
            ));
        }
        let mut transcript: Transcript = serde_json::from_reader(
            fs::File::open(&canonical).map_err(io_failed)?,
        )
        .map_err(|_| {
            ApiError::failed("the saved transcript is not valid JSON; run media_transcribe again")
        })?;
        transcript.filter(start, end, keyword)?;
        serde_json::to_value(transcript)
            .map_err(|_| ApiError::failed("could not encode the transcript"))
    }
}

fn reporter(events: EventSink, job: &Started) -> Box<dyn FnMut(JobStage) + Send> {
    let job = job.clone();
    Box::new(move |stage| {
        let (stage, fraction) = match stage {
            JobStage::Reading => ("reading audio", 0.0),
            JobStage::Loading => ("loading model", 0.0),
            JobStage::Working { done, .. } => ("transcribing", done),
        };
        events(Event::TranscriptProgress {
            job: job.job.clone(),
            path: job.path.clone(),
            stage: stage.to_owned(),
            fraction,
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JobState, Reply, Request, Response};
    use concat_host::AppDirs;
    use concat_speech::transcribe::Word;
    use std::sync::mpsc;
    use std::time::Duration;

    fn setup(events: EventSink) -> (Api, tempfile::TempDir, String) {
        let scratch = tempfile::tempdir().expect("scratch");
        let root = scratch.path().canonicalize().expect("canonical test path");
        let mut api = Api::with_dirs(
            AppDirs {
                config: root.join("config"),
                data: root.join("data"),
            },
            events,
        );
        api.restrict_writes_to(vec![root.clone()]);
        let path = root.join("Cut").to_str().expect("path").to_owned();
        assert!(matches!(
            api.dispatch(Request::ProjectCreate {
                location: root.to_str().expect("root").to_owned(),
                name: "Cut".to_owned(),
                video: None,
            }),
            Response::Result(_)
        ));
        (api, scratch, path)
    }

    #[test]
    fn missing_model_is_a_structured_error_without_creating_the_model_directory() {
        let (mut api, _scratch, path) = setup(Arc::new(|_| {}));
        let engine = Arc::new(Transcriber::new());
        api.share_transcriber(Arc::clone(&engine));
        let slot = engine.begin_timed().expect("claim engine");
        assert_eq!(api.ai_status()["transcription"]["busy"], true);
        drop(slot);
        let status = api.ai_status();
        assert_eq!(status["transcription"]["busy"], false);
        assert_eq!(status["transcription"]["installed"], false);
        assert!(status["speechGeneration"]["installed"].is_boolean());
        let Response::Error(error) = api.dispatch(Request::MediaTranscribe {
            path,
            media_id: Some("m1".to_owned()),
            clip_id: None,
            language: Some("zh".to_owned()),
            model_id: Some("base".to_owned()),
        }) else {
            panic!("expected missing model")
        };
        let error = serde_json::to_value(error).expect("json error");
        assert_eq!(error["code"], "modelMissing");
        let message = error["message"].as_str().expect("message");
        assert!(
            message.contains("model base")
                && message.contains("147951465 bytes")
                && message.contains("Settings > Transcriber")
        );
        assert!(!api.dirs.data.join("whisper-models").exists());
    }

    #[test]
    fn transcription_job_reports_progress_and_cancel_does_not_stop_another_job() {
        let (events_tx, events_rx) = mpsc::channel();
        let (mut api, _scratch, path) = setup(Arc::new(move |e| {
            events_tx.send(e).expect("event");
        }));
        let (resume_tx, resume_rx) = mpsc::channel();
        let window_slot = api.exporter().begin().expect("unrelated busy exporter");
        let started = api
            .start_transcript_job(
                path,
                "ignored.json".to_owned(),
                Arc::new(AtomicBool::new(false)),
                move |mut progress, stop| {
                    progress(JobStage::Working {
                        done: 0.375,
                        next: 0.5,
                    });
                    resume_rx
                        .recv_timeout(Duration::from_secs(5))
                        .expect("resume");
                    assert!(stop.load(Ordering::Relaxed));
                    Err(cancelled())
                },
            )
            .expect("job started");
        assert!(matches!(
            events_rx.recv_timeout(Duration::from_secs(5)),
            Ok(Event::TranscriptProgress { .. })
        ));
        let poll = |api: &mut Api| match api.dispatch(Request::ExportStatus {
            job: started.job.clone(),
        }) {
            Response::Result(Reply::Job(job)) => job,
            _ => panic!("job missing"),
        };
        assert_eq!(poll(&mut api).stage.as_deref(), Some("transcribing"));
        assert_eq!(poll(&mut api).fraction, 0.375);
        assert!(matches!(
            api.dispatch(Request::ExportCancel {
                job: started.job.clone()
            }),
            Response::Result(_)
        ));
        assert!(!window_slot.cancelled());
        resume_tx.send(()).expect("resume worker");
        assert!(matches!(
            events_rx.recv_timeout(Duration::from_secs(5)),
            Ok(Event::TranscriptFailed { .. })
        ));
        assert_eq!(poll(&mut api).state, JobState::Cancelled);
        api.finish();
    }

    #[test]
    fn transcript_query_reads_persisted_source_times_with_both_filters() {
        let (mut api, _scratch, path) = setup(Arc::new(|_| {}));
        let target = Target::Clip("c12".to_owned());
        let word = |start, end, text: &str| Word {
            start,
            end,
            text: text.to_owned(),
        };
        let transcript = Transcript {
            media_id: "m1".to_owned(),
            clip_id: Some("c12".to_owned()),
            audio_stream: Some(1),
            model_id: "tiny".to_owned(),
            source_start: 20.0,
            source_end: 35.0,
            segments: vec![
                TimedSegment {
                    start: 22.0,
                    end: 24.0,
                    text: "Hello café".to_owned(),
                    words: vec![word(22.0, 22.5, "Hello"), word(22.5, 24.0, "café")],
                },
                TimedSegment {
                    start: 24.0,
                    end: 30.0,
                    text: "Goodbye".to_owned(),
                    words: vec![word(24.0, 30.0, "Goodbye")],
                },
            ],
        };
        let output = target.output(Path::new(&path));
        let started = api
            .start_transcript_job(
                path.clone(),
                output.display().to_string(),
                Arc::new(AtomicBool::new(false)),
                move |_progress, cancel| transcript.save(&output, cancel),
            )
            .expect("job started");
        api.finish();
        assert_eq!(
            api.job_status(&started.job).expect("status").state,
            JobState::Done
        );
        let Response::Result(Reply::Transcript(found)) = api.dispatch(Request::TranscriptQuery {
            path,
            media_id: None,
            clip_id: Some("c12".to_owned()),
            start: Some(22.5),
            end: None,
            keyword: Some("CAFÉ".to_owned()),
        }) else {
            panic!("query failed")
        };
        assert_eq!(found["segments"].as_array().expect("segments").len(), 1);
        assert_eq!(
            found["segments"][0]["words"],
            json!([{ "start": 22.5, "end": 24.0, "text": "café" }])
        );
        assert_eq!(found["audioStream"], 1);
    }
}
