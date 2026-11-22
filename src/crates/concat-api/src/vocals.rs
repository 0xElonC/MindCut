// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! Prepare the same Spleeter cache the window and exporter consume.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use concat_project::Command;
use concat_project::model::VoiceKind;
use concat_speech::{isolate::Stems, vocals};
use serde_json::{Value, json};

use crate::{Api, ApiError, ErrorCode, Event, Holder, JobStatus, Started, key_of, not_open};

const STEMS: [&str; 2] = ["voice.wav", "rest.wav"];

fn requires_spleeter(command: &Command) -> bool {
    match command {
        Command::SetClipVoice { voice, .. } => {
            voice.is_some_and(|mix| mix.kind == VoiceKind::Vocals)
        }
        Command::Batch { commands } => commands.iter().any(requires_spleeter),
        _ => false,
    }
}

fn local_networks(data: &Path) -> Result<vocals::Networks, ApiError> {
    vocals::Networks::installed(data).ok_or_else(|| {
        let models = vocals::model_status(data);
        let missing: Vec<_> = models.iter().filter(|(_, _, on_disk)| !on_disk)
            .map(|(id, _, _)| *id).collect();
        let size = models.iter().map(|(_, size, _)| size).sum::<u64>();
        let size = format!("{},{:03},{:03}", size / 1_000_000, size / 1_000 % 1_000, size % 1_000);
        ApiError::new(
            ErrorCode::ModelMissing,
            format!(
                "Missing Spleeter model(s): {}. The pair ({}, {}) totals {size} bytes. In the window inspector, select Voice Isolation > Song (人声分离 → 歌曲) to download, then retry.",
                missing.join(", "), models[0].0, models[1].0,
            ),
        )
    })
}

fn io_error(error: std::io::Error) -> ApiError {
    ApiError::failed(format!("vocal separation cache: {error}"))
}

fn cancelled() -> ApiError {
    ApiError::new(ErrorCode::Cancelled, "vocal separation cancelled")
}

fn real_directory(dir: &Path) -> Result<(), ApiError> {
    if dir.canonicalize().ok().as_deref() != Some(dir) || !dir.is_dir() {
        return Err(ApiError::new(
            ErrorCode::Refused,
            "vocal separation cache must use real directories inside the project",
        ));
    }
    Ok(())
}

fn ensure_directory(dir: &Path) -> Result<(), ApiError> {
    match fs::create_dir(dir) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(io_error(error)),
    }
    real_directory(dir)
}

fn ready(dir: &Path) -> bool {
    STEMS
        .iter()
        .all(|file| fs::symlink_metadata(dir.join(file)).is_ok_and(|m| m.is_file() && m.len() > 0))
}

fn check_cache_path(project: &Path, target: &Path) -> Result<(), ApiError> {
    let parent = project.join("cache").join("voice");
    debug_assert_eq!(target.parent(), Some(parent.as_path()));
    real_directory(project)?;
    real_directory(&project.join("cache"))?;
    real_directory(&parent)?;
    match fs::symlink_metadata(target) {
        Ok(_) => real_directory(target),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

fn build_cache(
    project: &Path,
    target: &Path,
    cancel: &AtomicBool,
    work: impl FnOnce(&Stems) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    check_cache_path(project, target)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(cancelled());
    }
    if ready(target) {
        return Ok(());
    }
    let scratch = tempfile::Builder::new()
        .prefix(".split-")
        .tempdir_in(project.join("cache").join("voice"))
        .map_err(io_error)?;
    let result = work(&Stems {
        voice: scratch.path().join(STEMS[0]),
        rest: scratch.path().join(STEMS[1]),
    });
    if cancel.load(Ordering::Relaxed) {
        return Err(cancelled());
    }
    result?;
    if !ready(scratch.path()) {
        return Err(ApiError::failed(
            "vocal separation did not produce both audio stems",
        ));
    }
    check_cache_path(project, target)?;
    if ready(target) {
        return Ok(()); // The window may have completed the same split meanwhile.
    }
    // Publish a new pair together. When retrying an incomplete window cache,
    // replace its files only after both new stems have been produced.
    if !target.exists() {
        match fs::rename(scratch.path(), target) {
            Ok(()) => return Ok(()),
            Err(error) if !target.exists() => return Err(io_error(error)),
            Err(_) => check_cache_path(project, target)?,
        }
    }
    if !ready(target) {
        for file in STEMS {
            fs::rename(scratch.path().join(file), target.join(file)).map_err(io_error)?;
        }
    }
    Ok(())
}

impl Api {
    pub(crate) fn vocal_status(&self) -> Value {
        let models = vocals::model_status(&self.dirs.data);
        json!({
            "status": "available",
            "installed": models.iter().all(|(_, _, installed)| *installed),
            "busy": self.vocal_separator.is_busy(),
            "modelsDir": vocals::models_dir(&self.dirs.data),
            "models": models.map(|(id, size, installed)| json!({
                "id": id, "sizeBytes": size, "installed": installed,
            })),
        })
    }

    pub(crate) fn preflight_vocals(&self, command: &Command) -> Result<(), ApiError> {
        if requires_spleeter(command) {
            local_networks(&self.dirs.data)?;
        }
        Ok(())
    }

    pub(crate) fn separate_vocals(
        &mut self,
        path: &str,
        media_id: Option<&str>,
        clip_id: Option<&str>,
        live: bool,
    ) -> Result<Started, ApiError> {
        if media_id.is_some() == clip_id.is_some() {
            return Err(ApiError::invalid(
                "supply exactly one of mediaId and clipId",
            ));
        }
        let session = self.session(path)?;
        if key_of(session.path()) != key_of(path)
            || (!live && self.open.holder(path) != Some(Holder::Api))
        {
            return Err(not_open(path));
        }
        let networks = local_networks(&self.dirs.data)?;
        let project = session.project();
        let (media_id, stream) = if let Some(clip_id) = clip_id {
            let clip = project
                .timelines
                .iter()
                .flat_map(|tl| &tl.clips)
                .find(|clip| clip.id == clip_id)
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "clip is not in the project"))?;
            (clip.media_id.as_str(), clip.audio_stream)
        } else {
            (media_id.expect("one ID supplied"), None)
        };
        let media = project
            .media_by_id(media_id)
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "media is not in the project"))?;
        if !media.has_audio || media.placeholder {
            return Err(ApiError::invalid("source media has no audio"));
        }
        let source = fs::canonicalize(&media.path)
            .ok()
            .filter(|path| path.is_file())
            .ok_or_else(|| {
                ApiError::new(ErrorCode::NotFound, "source must be a local media file")
            })?;
        let root = Path::new(session.path()).canonicalize().map_err(io_error)?;
        let target = concat_export::voice_stems_dir(&root, &media.path, stream, VoiceKind::Vocals);
        let workers = vocals::workers_for(concat_host::memory::total());
        self.start_vocals_job(root, target, move |stems, cancel, progress| {
            vocals::separate(
                &source,
                stream.map(|n| n as usize),
                &networks,
                stems,
                workers,
                cancel,
                progress,
            )
            .map_err(ApiError::failed)
        })
    }

    fn start_vocals_job(
        &mut self,
        project: PathBuf,
        target: PathBuf,
        work: impl FnOnce(
            &Stems,
            &AtomicBool,
            concat_speech::isolate::Progress<'_>,
        ) -> Result<(), ApiError>
        + Send
        + 'static,
    ) -> Result<Started, ApiError> {
        let utf8 = || ApiError::invalid("vocal separation cache path is not UTF-8");
        let path = project.to_str().ok_or_else(utf8)?;
        let output = target.to_str().ok_or_else(utf8)?;
        self.writable(path)?;
        self.writable(output)?;
        let run = self
            .vocal_separator
            .begin("vocal separation")
            .map_err(|error| ApiError::new(ErrorCode::Busy, error))?;
        real_directory(&project)?;
        ensure_directory(&project.join("cache"))?;
        ensure_directory(&project.join("cache").join("voice"))?;
        check_cache_path(&project, &target)?;
        let started = Started {
            job: self.jobs.mint(),
            path: path.to_owned(),
            output: output.to_owned(),
        };
        let job = started.clone();
        let events = Arc::clone(&self.events);
        self.book.start(
            JobStatus::running(&job.job, &job.path, &job.output),
            run.cancel_handle(),
        );
        let thread = std::thread::Builder::new()
            .name(format!("vocals {}", job.job))
            .spawn(move || {
                let mut last = -1.0f32;
                let mut progress = |fraction: f32| {
                    if fraction - last >= 0.01 {
                        last = fraction;
                        events(Event::VocalsProgress {
                            job: job.job.clone(),
                            path: job.path.clone(),
                            fraction,
                        });
                    }
                };
                let cancel = run.cancel_flag();
                let result = build_cache(&project, &target, cancel, |stems| {
                    work(stems, cancel, &mut progress)
                });
                drop(run);
                events(match result {
                    Ok(()) => Event::VocalsDone {
                        job: job.job,
                        path: job.path,
                        output: job.output,
                    },
                    Err(error) => Event::VocalsFailed {
                        job: job.job,
                        path: job.path,
                        error,
                    },
                });
            })
            .map_err(|error| {
                self.book.abandon(&started.job);
                ApiError::failed(format!("could not start vocal separation: {error}"))
            })?;
        self.jobs.standalone.insert(started.job.clone());
        self.jobs.threads.push((started.job.clone(), thread));
        Ok(started)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventSink, JobState, Reply, Request, Response};
    use concat_host::AppDirs;
    use concat_project::model::VoiceMix;
    use std::sync::mpsc;
    use std::time::Duration;

    fn setup(events: EventSink) -> (Api, tempfile::TempDir, PathBuf) {
        let scratch = tempfile::tempdir().expect("scratch");
        let root = scratch.path().canonicalize().expect("canonical root");
        let mut api = Api::with_dirs(AppDirs::under(&root.join("app-data")), events);
        api.restrict_writes_to(vec![root.clone()]);
        api.create(root.to_str().unwrap(), "Cut", Default::default())
            .expect("project");
        (api, scratch, root.join("Cut"))
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

    fn write_stems(stems: &Stems) -> Result<(), ApiError> {
        fs::write(&stems.voice, wav()).map_err(io_error)?;
        fs::write(&stems.rest, wav()).map_err(io_error)
    }

    fn voice_command(id: &str) -> Command {
        Command::SetClipVoice {
            clip_id: id.to_owned(),
            voice: Some(VoiceMix {
                kind: VoiceKind::Vocals,
                ..VoiceMix::KEEP_VOICE
            }),
        }
    }

    #[test]
    fn missing_spleeter_has_the_same_error_in_the_job_and_edit_preflight() {
        let (mut api, _scratch, project) = setup(Arc::new(|_| {}));
        let path = project.to_str().unwrap().to_owned();
        let Response::Error(missing) = api.dispatch(Request::AudioSeparateVocals {
            path: path.clone(),
            media_id: None,
            clip_id: Some("clip".to_owned()),
        }) else {
            panic!("expected missing models")
        };
        assert_eq!(missing.code, ErrorCode::ModelMissing);
        for part in [
            "spleeter-2stems-vocals",
            "spleeter-2stems-accompaniment",
            "39,362,041 bytes",
            "人声分离 → 歌曲",
        ] {
            assert!(missing.message.contains(part), "{}", missing.message);
        }
        for command in [
            voice_command("clip"),
            Command::Batch {
                commands: vec![Command::Batch {
                    commands: vec![voice_command("clip")],
                }],
            },
        ] {
            let Response::Error(error) = api.dispatch(Request::EditApply {
                path: path.clone(),
                command: Box::new(command),
            }) else {
                panic!("an API edit must not trigger the window's download")
            };
            assert_eq!(error, missing);
        }
        assert!(
            api.preflight_vocals(&Command::SetClipVoice {
                clip_id: "clip".into(),
                voice: Some(VoiceMix::KEEP_VOICE)
            })
            .is_ok()
        );
        assert!(!vocals::models_dir(&api.dirs.data).exists());
        assert_eq!(api.ai_status()["vocalSeparation"]["installed"], false);
        assert_eq!(api.running(), 0);
    }

    #[test]
    fn cache_job_reports_progress_finishes_with_two_stems_and_reuses_them() {
        let (sent, received) = mpsc::channel();
        let (mut api, _scratch, project) = setup(Arc::new(move |event| {
            let _ = sent.send(event);
        }));
        let cache =
            concat_export::voice_stems_dir(&project, "song.wav", Some(2), VoiceKind::Vocals);
        let (resume, wait) = mpsc::channel();
        let started = api
            .start_vocals_job(project.clone(), cache.clone(), move |stems, _, report| {
                report(0.5);
                wait.recv_timeout(Duration::from_secs(5))
                    .expect("resume fixture");
                write_stems(stems)
            })
            .expect("start split");
        assert!(matches!(
            received.recv_timeout(Duration::from_secs(5)),
            Ok(Event::VocalsProgress { .. })
        ));
        let running = api.job_status(&started.job).expect("running");
        assert_eq!(running.state, JobState::Running);
        assert_eq!(running.fraction, 0.5);
        assert_eq!(api.vocal_status()["busy"], true);
        assert!(!cache.exists());
        resume.send(()).expect("resume");
        api.finish();
        let done = api.job_status(&started.job).expect("done");
        assert_eq!(done.state, JobState::Done);
        assert_eq!(Path::new(&done.output), cache);
        assert!(matches!(
            received.recv_timeout(Duration::from_secs(5)),
            Ok(Event::VocalsDone { .. })
        ));
        assert!(ready(&cache));
        assert_eq!(api.vocal_status()["busy"], false);
        let cached = api
            .start_vocals_job(project, cache, |_, _, _| {
                panic!("the existing cache must be reused")
            })
            .expect("cached job");
        api.finish();
        assert_eq!(api.job_status(&cached.job).unwrap().state, JobState::Done);
    }

    #[test]
    fn cancelling_removes_partial_stems() {
        let (sent, received) = mpsc::channel();
        let (mut api, _scratch, project) = setup(Arc::new(move |event| {
            let _ = sent.send(event);
        }));
        let cache = concat_export::voice_stems_dir(&project, "song.wav", None, VoiceKind::Vocals);
        let (resume, wait) = mpsc::channel();
        let started = api
            .start_vocals_job(project, cache.clone(), move |stems, cancel, report| {
                fs::write(&stems.voice, wav()).map_err(io_error)?;
                report(0.25);
                wait.recv_timeout(Duration::from_secs(5))
                    .expect("resume fixture");
                assert!(cancel.load(Ordering::Relaxed));
                Ok(())
            })
            .expect("start split");
        assert!(matches!(
            received.recv_timeout(Duration::from_secs(5)),
            Ok(Event::VocalsProgress { .. })
        ));
        api.cancel(&started.job).expect("cancel");
        resume.send(()).expect("resume");
        api.finish();
        assert_eq!(
            api.job_status(&started.job).unwrap().state,
            JobState::Cancelled
        );
        assert_eq!(fs::read_dir(cache.parent().unwrap()).unwrap().count(), 0);
    }

    #[test]
    fn tool_uses_exporters_cache_and_the_clip_mix_can_then_be_applied() {
        let (mut api, scratch, project) = setup(Arc::new(|_| {}));
        let path = project.to_str().unwrap();
        let source = scratch.path().canonicalize().unwrap().join("song.wav");
        fs::write(&source, wav()).unwrap();
        let source = source.to_str().unwrap();
        let media = api
            .import(path, source)
            .expect("import WAV")
            .created_id
            .unwrap();
        let view = api
            .apply(
                path,
                Command::AddClipAtFirstFree {
                    media_id: media,
                    start: 0.0,
                },
            )
            .expect("add clip");
        let clip_id = view.project.active().clips[0].id.clone();
        let models = vocals::models_dir(&api.dirs.data);
        fs::create_dir_all(&models).unwrap();
        fs::write(
            models.join("spleeter-2stems-vocals.fp16.onnx"),
            b"not a real model",
        )
        .unwrap();
        assert_eq!(api.vocal_status()["installed"], false);
        fs::write(
            models.join("spleeter-2stems-accompaniment.fp16.onnx"),
            b"not a real model",
        )
        .unwrap();
        assert_eq!(api.vocal_status()["installed"], true);
        let media_path = &view.project.media[0].path;
        let cache = concat_export::voice_stems_dir(
            &project,
            media_path,
            view.project.active().clips[0].audio_stream,
            VoiceKind::Vocals,
        );
        fs::create_dir_all(&cache).unwrap();
        write_stems(&Stems {
            voice: cache.join(STEMS[0]),
            rest: cache.join(STEMS[1]),
        })
        .unwrap();
        let Response::Result(Reply::Started(started)) =
            api.dispatch(Request::AudioSeparateVocals {
                path: path.to_owned(),
                media_id: None,
                clip_id: Some(clip_id.clone()),
            })
        else {
            panic!("start cached tool call")
        };
        api.finish();
        assert_eq!(api.job_status(&started.job).unwrap().state, JobState::Done);
        assert_eq!(Path::new(&started.output), cache);
        assert!(
            api.session(path).unwrap().project().active().clips[0]
                .voice
                .is_none()
        );
        let view = api.apply(path, voice_command(&clip_id)).expect("apply mix");
        assert_eq!(
            view.project.active().clips[0].voice.unwrap().kind,
            VoiceKind::Vocals
        );
    }
}
