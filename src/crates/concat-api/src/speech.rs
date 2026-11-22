// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! Background speech synthesis. The dispatcher imports each finished WAV
//! on the same thread as any other project edit, before saying it is done.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use concat_host::media;
use concat_project::{Command, commands::NewMedia};
use concat_speech::JobStage;
use concat_speech::tts::{self, Family, ModelStatus, SpeakRequest, Speech};
use serde_json::{Value, json};
use tempfile::NamedTempFile;

use crate::{Api, ApiError, ErrorCode, Event, JobStatus, Live, Started, key_of, not_open, refused};

type Progress = Box<dyn FnMut(JobStage) + Send>;

pub(crate) struct Pending {
    job: Started,
    cancel: Arc<AtomicBool>,
    live: Option<Arc<dyn Live>>,
    worker: JoinHandle<Result<Generated, ApiError>>,
}

impl Pending {
    pub(crate) fn id(&self) -> &str {
        &self.job.job
    }
}

struct Generated {
    file: NamedTempFile,
    item: NewMedia,
    duration: f64,
}

fn cancelled() -> ApiError {
    ApiError::new(ErrorCode::Cancelled, "speech generation cancelled")
}

fn unavailable(message: &str) -> ApiError {
    ApiError::new(ErrorCode::VoiceUnavailable, message)
}

fn stock_model(voice_id: i32, models: &[ModelStatus]) -> Result<&ModelStatus, ApiError> {
    if tts::is_clone(voice_id) {
        return Err(unavailable(
            "cloned voices are unavailable; choose a stock voice from speech_voices",
        ));
    }
    let family = tts::voice_family(voice_id)
        .ok_or_else(|| unavailable("unknown voiceId; choose a voice from speech_voices"))?;
    if family == Family::System && !tts::has_offline_system_voice() {
        return Err(unavailable(
            "no offline system voice is available on this device",
        ));
    }
    let model = models
        .iter()
        .find(|model| model.family == family)
        .ok_or_else(|| unavailable("this build cannot use that voice; see speech_voices"))?;
    if family != Family::System && !model.downloaded {
        return Err(ApiError::new(
            ErrorCode::ModelMissing,
            format!(
                "Speech model {} is not installed (approximately {} bytes). Download it in Settings > Speech, then retry.",
                model.id, model.size_bytes,
            ),
        ));
    }
    Ok(model)
}

fn audio_error(error: std::io::Error) -> ApiError {
    ApiError::failed(format!("project audio: {error}"))
}

fn check_audio_folder(project: &Path) -> Result<(), ApiError> {
    let audio = project.join("audio");
    if project.canonicalize().ok().as_deref() != Some(project)
        || audio.canonicalize().ok().as_deref() != Some(audio.as_path())
        || !audio.is_dir()
    {
        return Err(ApiError::new(
            ErrorCode::Refused,
            "speech audio folder must be a real directory inside the project",
        ));
    }
    Ok(())
}

impl Api {
    pub(crate) fn speech_status(&self) -> Value {
        let status = Speech::status_read_only(&self.dirs);
        let offline = tts::has_offline_system_voice();
        let models: Vec<_> = status
            .models
            .iter()
            .filter(|model| model.family != Family::System)
            .collect();
        let synthesizing = self.speech.is_busy();
        let downloading = self.speech.is_downloading();
        json!({
            "status": if offline || !models.is_empty() { "available" } else { "unavailable" },
            "installed": offline || models.iter().any(|model| model.downloaded),
            "busy": synthesizing || downloading,
            "synthesizing": synthesizing,
            "downloading": downloading,
            "systemVoiceAvailable": offline,
            "modelsDir": status.models_dir,
            "models": models,
        })
    }

    pub(crate) fn speech_voices(&self) -> Value {
        let status = Speech::status_read_only(&self.dirs);
        let mut voices: Vec<Value> = status
            .voices
            .iter()
            .filter_map(|voice| {
                if tts::is_clone(voice.id) || voice.family == Family::System {
                    return None;
                }
                let model = status
                    .models
                    .iter()
                    .find(|model| model.family == voice.family)?;
                Some(json!({
                    "id": voice.id, "name": voice.name, "family": voice.family, "isClone": false,
                    "modelId": model.id, "requiresModel": true, "modelInstalled": model.downloaded,
                    "available": model.downloaded,
                }))
            })
            .collect();
        let offline = tts::has_offline_system_voice();
        for &(id, name) in tts::SYSTEM_VOICES {
            voices.push(json!({
                "id": id, "name": name, "family": Family::System, "isClone": false,
                "modelId": tts::SYSTEM_ID, "requiresModel": false, "available": offline,
            }));
        }
        json!({ "voices": voices })
    }

    pub(crate) fn generate_speech(
        &mut self,
        path: &str,
        text: String,
        voice_id: i32,
        speed: Option<f32>,
    ) -> Result<Started, ApiError> {
        if text.trim().is_empty() {
            return Err(ApiError::invalid("speech text is empty"));
        }
        let speed = speed.unwrap_or(1.0);
        if !(0.5..=2.0).contains(&speed) {
            return Err(ApiError::invalid("speed must be between 0.5 and 2"));
        }
        let status = Speech::status_read_only(&self.dirs);
        let model = stock_model(voice_id, &status.models)?;
        let dirs = self.dirs.clone();
        let run = self
            .speech
            .begin_stock()
            .map_err(|error| ApiError::new(ErrorCode::Busy, error))?;
        let cancel = run.cancel_handle();
        let project = self.session(path)?.path().to_owned();
        let request = SpeakRequest {
            model_id: model.id.clone(),
            voice: voice_id,
            reference: None,
            text,
            speed,
            project,
        };
        self.start_speech_job(path, cancel, move |target, progress| {
            // The engine can return platform/provider errors. Keep any caller
            // text those errors may contain out of events and transport logs.
            run.speak(&dirs, &request, target, progress)
                .map(|_| ())
                .map_err(|_| {
                    ApiError::failed(
                        "local speech synthesis failed; check the voice model and try again",
                    )
                })
        })
    }

    fn start_speech_job(
        &mut self,
        path: &str,
        cancel: Arc<AtomicBool>,
        speak: impl FnOnce(&Path, Progress) -> Result<(), ApiError> + Send + 'static,
    ) -> Result<Started, ApiError> {
        let project = Path::new(self.session(path)?.path())
            .canonicalize()
            .map_err(audio_error)?;
        let path = project
            .to_str()
            .ok_or_else(|| ApiError::invalid("project path is not UTF-8"))?;
        let live = self.live_at(path);
        let audio = project.join("audio");
        self.writable(path)?;
        self.writable(
            audio
                .to_str()
                .ok_or_else(|| ApiError::invalid("audio path is not UTF-8"))?,
        )?;
        match std::fs::create_dir(&audio) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(audio_error(error)),
        }
        check_audio_folder(&project)?;
        let file = tempfile::Builder::new()
            .prefix("speech-")
            .suffix(".wav")
            .tempfile_in(&audio)
            .map_err(audio_error)?;
        let job = Started {
            job: self.jobs.mint(),
            path: path.to_owned(),
            output: file.path().to_string_lossy().into_owned(),
        };
        let work_job = job.clone();
        let work_cancel = Arc::clone(&cancel);
        let events = Arc::clone(&self.events);
        self.book.start(
            JobStatus::running(&job.job, &job.path, &job.output),
            Arc::clone(&cancel),
        );
        let worker = std::thread::Builder::new()
            .name(format!("speech {}", job.job))
            .spawn(move || {
                let progress = Box::new(move |stage| {
                    let (stage, fraction) = match stage {
                        JobStage::Reading => ("reading voice", 0.0),
                        JobStage::Loading => ("loading voice model", 0.0),
                        JobStage::Working { done, .. } => ("synthesizing speech", done),
                    };
                    events(Event::SpeechProgress {
                        job: work_job.job.clone(),
                        path: work_job.path.clone(),
                        stage: stage.to_owned(),
                        fraction,
                    });
                });
                if work_cancel.load(Ordering::Relaxed) {
                    return Err(cancelled());
                }
                check_audio_folder(&project)?;
                let outcome = speak(file.path(), progress);
                if work_cancel.load(Ordering::Relaxed) {
                    return Err(cancelled());
                }
                outcome?;
                let summary =
                    media::probe(file.path().to_str().expect("WAV path in UTF-8 project"))
                        .map_err(ApiError::failed)?;
                let item = summary.to_new_media();
                let duration = item
                    .duration
                    .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
                    .ok_or_else(|| ApiError::failed("speech output has no audio duration"))?;
                if !item.has_audio {
                    return Err(ApiError::failed("speech output contains no audio"));
                }
                Ok(Generated {
                    file,
                    item,
                    duration,
                })
            })
            .map_err(|error| {
                self.book.abandon(&job.job);
                ApiError::failed(format!("could not start the speech job: {error}"))
            })?;
        self.speech_jobs.push(Pending {
            job: job.clone(),
            cancel,
            live,
            worker,
        });
        Ok(job)
    }

    /// The worker does not edit the session. On the next request (normally
    /// a status poll) the API imports its output before publishing completion.
    pub(crate) fn reap_speech(&mut self, wait: bool) {
        let mut index = 0;
        while index < self.speech_jobs.len() {
            if !wait && !self.speech_jobs[index].worker.is_finished() {
                index += 1;
                continue;
            }
            let Pending {
                worker,
                job,
                cancel,
                live,
            } = self.speech_jobs.swap_remove(index);
            let outcome = worker.join().unwrap_or_else(|_| {
                Err(ApiError::failed("speech worker stopped without a result"))
            });
            let result = if cancel.load(Ordering::Relaxed) {
                Err(cancelled())
            } else {
                outcome.and_then(|written| self.import_speech(&job.path, live.as_deref(), written))
            };
            (self.events)(match result {
                Ok((media_id, duration)) => Event::SpeechDone {
                    job: job.job,
                    path: job.path,
                    output: job.output,
                    media_id,
                    duration,
                },
                Err(error) => Event::SpeechFailed {
                    job: job.job,
                    path: job.path,
                    error,
                },
            });
        }
    }

    fn import_speech(
        &mut self,
        path: &str,
        live: Option<&dyn Live>,
        mut written: Generated,
    ) -> Result<(String, f64), ApiError> {
        check_audio_folder(Path::new(path))?;
        self.writable(written.file.path().to_str().expect("already checked"))?;
        let command = Command::AddMedia { item: written.item };
        let view = if let Some(live) = live {
            live.apply_to(path, command).map_err(refused)?
        } else {
            let session = self
                .sessions
                .get_mut(&key_of(path))
                .ok_or_else(|| not_open(path))?;
            session.apply(command).map_err(refused)?
        };
        let id = view
            .created_id
            .or_else(|| {
                view.project
                    .media
                    .into_iter()
                    .find(|item| Path::new(&item.path) == written.file.path())
                    .map(|item| item.id)
            })
            .ok_or_else(|| ApiError::failed("speech audio was not imported"))?;
        // Disable the automatic unlink only after the project accepted the edit.
        written.file.disable_cleanup(true);
        Ok((id, written.duration))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JobState, Reply, Request, Response};
    use concat_host::AppDirs;
    use std::sync::mpsc;
    use std::time::Duration;

    fn setup(events: Arc<dyn Fn(Event) + Send + Sync>) -> (Api, tempfile::TempDir, String) {
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
        let path = root.join("Cut").to_str().expect("project path").to_owned();
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

    fn wav() -> Vec<u8> {
        let rate = 8_000u32;
        let size = rate * 2;
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&(36 + size).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 2).to_le_bytes());
        wav.extend_from_slice(b"\x02\0\x10\0data");
        wav.extend_from_slice(&size.to_le_bytes());
        wav.resize(wav.len() + size as usize, 0);
        wav
    }

    #[test]
    fn available_stock_voices_exclude_clones_and_validate_before_start() {
        let (mut api, _scratch, path) = setup(Arc::new(|_| {}));
        let voices = api.speech_voices();
        let voices = voices["voices"].as_array().expect("list of voices");
        assert!(
            voices
                .iter()
                .all(|voice| voice["id"] != tts::CHATTERBOX_CLONE && voice["isClone"] == false)
        );
        assert!(
            voices
                .iter()
                .any(|voice| voice["family"] == "system" && voice["requiresModel"] == false)
        );
        let mut request = |voice_id, text: &str| {
            let Response::Error(error) = api.dispatch(Request::SpeechGenerate {
                path: path.clone(),
                text: text.to_owned(),
                voice_id,
                speed: None,
            }) else {
                panic!("expected speech request to be refused")
            };
            error
        };
        assert_eq!(request(2000, "  ").code, ErrorCode::Invalid);
        assert_eq!(
            request(tts::CHATTERBOX_CLONE, "hello").code,
            ErrorCode::VoiceUnavailable
        );
        assert_eq!(request(-2, "hello").code, ErrorCode::VoiceUnavailable);
        if voices.iter().any(|voice| voice["id"] == 2000) {
            let error = request(2000, "hello");
            assert_eq!(error.code, ErrorCode::ModelMissing);
            assert!(
                error.message.contains("chatterbox-turbo-q8")
                    && error.message.contains("bytes")
                    && error.message.contains("Settings > Speech")
            );
        }
        assert!(!api.dirs.data.join("tts-models").exists());
    }

    #[test]
    fn generated_audio_is_probed_imported_and_never_placed_on_the_timeline() {
        let (tx, rx) = mpsc::channel();
        let (mut api, _scratch, path) = setup(Arc::new(move |event| {
            let _ = tx.send(event);
        }));
        let (resume, wait) = mpsc::channel();
        let started = api
            .start_speech_job(
                &path,
                Arc::new(AtomicBool::new(false)),
                move |file, mut progress| {
                    progress(JobStage::Working {
                        done: 0.25,
                        next: 0.75,
                    });
                    wait.recv_timeout(Duration::from_secs(5))
                        .expect("resume fake synth");
                    std::fs::write(file, wav()).map_err(audio_error)
                },
            )
            .expect("started");
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Event::SpeechProgress { .. })
        ));
        let running = api.job_status(&started.job).expect("running");
        assert_eq!(running.state, JobState::Running);
        assert_eq!(running.fraction, 0.25);
        assert!(running.media_id.is_none());
        resume.send(()).expect("resume");
        api.finish();
        let complete = api.job_status(&started.job).expect("done");
        assert_eq!(complete.state, JobState::Done, "{complete:?}");
        assert_eq!(complete.duration, Some(1.0));
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Event::SpeechDone { .. })
        ));
        let file = Path::new(&complete.output);
        assert!(file.is_file());
        assert_eq!(
            file.parent(),
            Some(Path::new(&path).join("audio").as_path())
        );
        let Response::Result(Reply::View(view)) = api.dispatch(Request::ProjectGet { path }) else {
            panic!("project")
        };
        assert_eq!(view.project.media.len(), 1);
        assert_eq!(
            view.project.media.first().map(|item| item.id.clone()),
            complete.media_id
        );
        assert!(view.project.active().clips.is_empty());
    }

    #[test]
    fn cancelling_a_speech_job_removes_the_unimported_file() {
        let (tx, rx) = mpsc::channel();
        let (mut api, _scratch, path) = setup(Arc::new(move |event| {
            let _ = tx.send(event);
        }));
        let (resume, wait) = mpsc::channel();
        let started = api
            .start_speech_job(
                &path,
                Arc::new(AtomicBool::new(false)),
                move |file, mut progress| {
                    progress(JobStage::Loading);
                    wait.recv_timeout(Duration::from_secs(5))
                        .expect("resume fake synth");
                    std::fs::write(file, wav()).map_err(audio_error)
                },
            )
            .expect("started");
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Event::SpeechProgress { .. })
        ));
        api.cancel(&started.job).expect("cancel");
        resume.send(()).expect("resume");
        api.finish();
        assert_eq!(
            api.job_status(&started.job).expect("end").state,
            JobState::Cancelled
        );
        assert!(!Path::new(&started.output).exists());
        assert!(
            api.session(&path)
                .expect("open project")
                .project()
                .media
                .is_empty()
        );
    }
}
