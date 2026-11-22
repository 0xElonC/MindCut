// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Concat contributors

//! Background enhancement, with the final edit on the API's project thread.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use concat_host::enhance::{self, EnhanceRequest, Progress};
use concat_project::model::{Clip, ClipKind, Project};
use serde_json::{Value, json};

use crate::{
    Api, ApiError, Command, ErrorCode, Event, Holder, JobStatus, Live, Started, key_of, media,
    not_open, refused,
};

const MARGIN: f64 = 1.0; // Match the inspector's trim margin.

struct Edit {
    timeline: String,
    clip: Clip,
    source: String,
    name: String,
    start: f64,
    offset: f64,
}

pub(crate) struct Pending {
    job: Started,
    edit: Edit,
    cancel: Arc<AtomicBool>,
    live: Option<Arc<dyn Live>>,
    worker: JoinHandle<Result<(), ApiError>>,
}

impl Pending {
    pub(crate) fn id(&self) -> &str {
        &self.job.job
    }
}

fn missing(data: &Path) -> ApiError {
    let (id, bytes, _) = enhance::local_model(data);
    ApiError::new(
        ErrorCode::ModelMissing,
        format!(
            "Missing {id} ({bytes} bytes). In the window's clip inspector, turn on Enhance and click Preview to download it."
        ),
    )
}

fn cancelled() -> ApiError {
    ApiError::new(ErrorCode::Cancelled, "clip enhancement cancelled")
}

fn active_clip<'a>(project: &'a Project, id: &str) -> Result<&'a Clip, ApiError> {
    project
        .active()
        .clip(id)
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "clip is not on the active timeline"))
}

fn source_id(clip: &Clip) -> &str {
    clip.enhanced
        .as_ref()
        .map_or(&clip.media_id, |link| &link.original)
}

fn check_cache(target: &Path) -> Result<bool, ApiError> {
    let dir = target.parent().expect("cache target always has a parent");
    if dir.canonicalize().ok().as_deref() != Some(dir) || !dir.is_dir() {
        return Err(refused(
            "enhancement cache must be a real directory inside the project".to_owned(),
        ));
    }
    match fs::symlink_metadata(target) {
        Ok(meta) if meta.is_file() && meta.len() > 0 => Ok(true),
        Ok(_) => Err(refused(
            "enhancement cache target must be a regular, nonempty file".to_owned(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ApiError::failed(error.to_string())),
    }
}

impl Api {
    pub(crate) fn enhancement_status(&self) -> Value {
        let (id, size, dir) = enhance::local_model(&self.dirs.data);
        let installed = self.enhancers.installed();
        json!({
            "status": "available", "installed": installed,
            "busy": self.enhancement.is_some(), "modelsDir": dir,
            "models": [{"id": id, "sizeBytes": size, "installed": installed}],
        })
    }

    pub(crate) fn enhance_clip(
        &mut self,
        path: &str,
        id: &str,
        asked: Option<f32>,
        live: Option<Arc<dyn Live>>,
    ) -> Result<Started, ApiError> {
        if asked.is_some_and(|scale| !(1.0..=4.0).contains(&scale)) {
            return Err(ApiError::invalid(
                "scale must be between 1 and 4; omit it for automatic scale",
            ));
        }
        let session = self.session(path)?;
        let project = session.project();
        let clip = active_clip(project, id)?;
        if !matches!(clip.kind, ClipKind::Video | ClipKind::Image) {
            return Err(ApiError::invalid(
                "enhancement requires a video or image clip",
            ));
        }
        let src = project
            .media
            .iter()
            .find(|item| item.id == source_id(clip))
            .ok_or_else(|| {
                ApiError::new(ErrorCode::NotFound, "original media is not in the project")
            })?;
        let (width, height) = (src.width.unwrap_or(0), src.height.unwrap_or(0));
        if src.placeholder || width == 0 || height == 0 {
            return Err(ApiError::invalid(
                "source must be a local media file with a picture",
            ));
        }
        let source = fs::canonicalize(&src.path)
            .ok()
            .filter(|path| path.is_file())
            .and_then(|path| path.into_os_string().into_string().ok())
            .ok_or_else(|| ApiError::invalid("source must be a local media file"))?;
        let root = Path::new(session.path())
            .canonicalize()
            .map_err(|error| ApiError::failed(error.to_string()))?;
        self.writable(
            root.to_str()
                .ok_or_else(|| ApiError::invalid("project path is not UTF-8"))?,
        )?;
        if !self.enhancers.installed() {
            return Err(missing(&self.dirs.data));
        }
        let scale = enhance::scale_of(asked.unwrap_or(0.0), width, height);
        let strength = enhance::DEFAULT_STRENGTH;
        let still = clip.kind == ClipKind::Image;
        let original_start = clip.source_start
            + clip
                .enhanced
                .as_ref()
                .filter(|link| link.on)
                .map_or(0.0, |link| link.offset);
        let span = (!still).then(|| {
            let covered = clip.duration * clip.speed;
            let from = (original_start - MARGIN).max(0.0);
            let to = (original_start + covered + MARGIN).min(src.duration.unwrap_or(f64::MAX));
            (from, (to - from).max(covered))
        });
        let offset = span.map_or(0.0, |(start, _)| start);
        let target = enhance::target_for(&root, &src.path, scale, strength, still, span)
            .ok_or_else(|| ApiError::failed("could not read source metadata"))?;
        let request = EnhanceRequest {
            media_path: source,
            still,
            scale,
            strength,
            target: target.clone(),
            span,
        };
        let edit = Edit {
            timeline: project.active_timeline_id.clone(),
            clip: clip.clone(),
            source: src.path.clone(),
            name: format!(
                "{} (enhanced {scale}x, {}%)",
                src.name,
                enhance::strength_percent(strength)
            ),
            start: (original_start - offset).max(0.0),
            offset,
        };
        let engines = Arc::clone(&self.enhancers);
        let data = self.dirs.data.clone();
        self.start_enhancement(path, target, edit, live, move |target, cancel, report| {
            // Check again for cache hits as well as new runs. The host's local
            // entry point independently enforces that it can never download.
            if !engines.installed() {
                return Err(missing(&data));
            }
            if check_cache(target)? {
                return Ok(());
            }
            // The window uses fixed partial names; isolate the API's writes
            // from those names and from any pre-existing files or symlinks.
            let scratch = tempfile::tempdir_in(target.parent().unwrap())
                .map_err(|error| ApiError::failed(error.to_string()))?;
            let mut request = request;
            request.target = scratch.path().join(target.file_name().unwrap());
            let result = engines.enhance_local(&request, cancel, &mut |progress| {
                if let Progress::Analysing(fraction) = progress {
                    report(fraction);
                }
            });
            let output = result.map_err(|error| {
                if error == enhance::MODEL_MISSING || !engines.installed() {
                    missing(&data)
                } else {
                    ApiError::failed(error)
                }
            })?;
            if cancel.load(Ordering::Relaxed) {
                return Err(cancelled());
            }
            if !check_cache(target)? {
                // No overwriting a result published by the window while this ran.
                match fs::hard_link(output, target) {
                    Ok(()) => {}
                    Err(error)
                        if error.kind() == std::io::ErrorKind::AlreadyExists
                            && check_cache(target)? => {}
                    Err(error) => return Err(ApiError::failed(error.to_string())),
                }
            }
            Ok(())
        })
    }

    fn start_enhancement(
        &mut self,
        path: &str,
        target: PathBuf,
        edit: Edit,
        live: Option<Arc<dyn Live>>,
        work: impl FnOnce(&Path, &AtomicBool, &mut dyn FnMut(f32)) -> Result<(), ApiError>
        + Send
        + 'static,
    ) -> Result<Started, ApiError> {
        if self.enhancement.is_some() {
            return Err(ApiError::new(
                ErrorCode::Busy,
                "a clip enhancement is already running",
            ));
        }
        let output = target
            .to_str()
            .ok_or_else(|| ApiError::invalid("cache path is not UTF-8"))?;
        self.writable(output)?;
        let cache = target.parent().expect("cache target always has a parent");
        match fs::create_dir(cache) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(ApiError::failed(error.to_string())),
        }
        check_cache(&target)?;
        let job = Started {
            job: self.jobs.mint(),
            path: path.to_owned(),
            output: output.to_owned(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.book.start(
            JobStatus::running(&job.job, &job.path, &job.output),
            Arc::clone(&cancel),
        );
        let thread_job = job.clone();
        let stop = Arc::clone(&cancel);
        let events = Arc::clone(&self.events);
        let worker = std::thread::Builder::new()
            .name(format!("enhance {}", job.job))
            .spawn(move || {
                let mut last = -1.0f32;
                work(&target, &stop, &mut |fraction| {
                    if fraction - last >= 0.01 {
                        last = fraction;
                        events(Event::EnhanceProgress {
                            job: thread_job.job.clone(),
                            path: thread_job.path.clone(),
                            fraction,
                        });
                    }
                })
            });
        let worker = worker.map_err(|error| {
            let error = ApiError::failed(error.to_string());
            (self.events)(Event::EnhanceFailed {
                job: job.job.clone(),
                path: job.path.clone(),
                error: error.clone(),
            });
            error
        })?;
        self.enhancement = Some(Pending {
            job: job.clone(),
            edit,
            cancel,
            live,
            worker,
        });
        Ok(job)
    }

    pub(crate) fn reap_enhancement(&mut self, wait: bool) {
        if !self
            .enhancement
            .as_ref()
            .is_some_and(|pending| wait || pending.worker.is_finished())
        {
            return;
        }
        let Pending {
            job,
            edit,
            cancel,
            live,
            worker,
        } = self.enhancement.take().unwrap();
        let outcome = worker.join().unwrap_or_else(|_| {
            Err(ApiError::failed(
                "enhancement worker stopped without a result",
            ))
        });
        let result = if cancel.load(Ordering::Relaxed) {
            Err(cancelled())
        } else {
            outcome.and_then(|()| self.adopt_enhancement(&job, edit, live.as_deref()))
        };
        (self.events)(match result {
            Ok(()) => Event::EnhanceDone {
                job: job.job,
                path: job.path,
                output: job.output,
            },
            Err(error) => Event::EnhanceFailed {
                job: job.job,
                path: job.path,
                error,
            },
        });
    }

    fn adopt_enhancement(
        &mut self,
        job: &Started,
        edit: Edit,
        live: Option<&dyn Live>,
    ) -> Result<(), ApiError> {
        self.writable(&job.output)?;
        if !check_cache(Path::new(&job.output))? {
            return Err(ApiError::failed("enhanced copy no longer exists"));
        }
        let mut item = media::probe(&job.output)
            .map_err(ApiError::failed)?
            .to_new_media();
        item.name = edit.name;
        let snapshot = if let Some(live) = live {
            if live
                .path()
                .is_none_or(|path| key_of(&path) != key_of(&job.path))
            {
                return Err(not_open(&job.path));
            }
            Some(live.snapshot().map_err(ApiError::failed)?)
        } else {
            if self.open.holder(&job.path) != Some(Holder::Api) {
                return Err(not_open(&job.path));
            }
            None
        };
        let session = match &snapshot {
            Some(session) => session,
            None => self.session(&job.path)?,
        };
        let project = session.project();
        let clip = active_clip(project, &edit.clip.id)?;
        if project.active_timeline_id != edit.timeline
            || *clip != edit.clip
            || !project
                .media
                .iter()
                .any(|item| item.id == source_id(clip) && item.path == edit.source)
            || item
                .duration
                .is_some_and(|length| edit.start + clip.duration * clip.speed > length + 1e-3)
        {
            return Err(refused(
                "clip or source changed during enhancement, or the copy is too short".to_owned(),
            ));
        }
        let id = edit.clip.id;
        let command = Command::ReplaceClipMedia {
            clip_id: id.clone(),
            item,
            source_start: Some(edit.start),
            enhanced: true,
            offset: edit.offset,
        };
        let view = if let Some(live) = live {
            live.apply_to(&key_of(&job.path), command)
        } else {
            self.session_mut(&job.path)?.apply(command)
        }
        .map_err(refused)?;
        let clip = active_clip(&view.project, &id)?;
        if !clip.enhanced.as_ref().is_some_and(|link| link.on)
            || !view
                .project
                .media
                .iter()
                .any(|item| item.id == clip.media_id && item.path == job.output)
        {
            return Err(refused(
                "enhanced copy was not applied to the clip".to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventSink, JobState, Reply, Request, Response, Session};
    use concat_host::AppDirs;
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;

    fn png(path: &Path) {
        let mut encoder = png::Encoder::new(fs::File::create(path).unwrap(), 4, 4);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[200; 48])
            .unwrap();
    }

    fn setup(events: EventSink) -> (Api, tempfile::TempDir, String, String, PathBuf) {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().canonicalize().unwrap();
        let mut api = Api::with_dirs(AppDirs::under(&root.join("app-data")), events);
        api.restrict_writes_to(vec![root.clone()]);
        api.create(root.to_str().unwrap(), "Cut", Default::default())
            .unwrap();
        let path = root.join("Cut").into_os_string().into_string().unwrap();
        let source = root.join("source.png");
        png(&source);
        let media_id = api
            .import(&path, source.to_str().unwrap())
            .unwrap()
            .project
            .media[0]
            .id
            .clone();
        let view = api
            .apply(
                &path,
                Command::AddClipAtFirstFree {
                    media_id,
                    start: 0.0,
                },
            )
            .unwrap();
        let id = view.project.active().clips[0].id.clone();
        (api, scratch, path, id, source)
    }

    fn request(path: &str, id: &str, scale: Option<f32>) -> Request {
        Request::ClipEnhance {
            path: path.to_owned(),
            clip_id: id.to_owned(),
            scale,
        }
    }

    #[test]
    fn absent_model_and_invalid_scale_do_not_start_a_job() {
        let (mut api, _scratch, path, id, _) = setup(Arc::new(|_| {}));
        for (scale, code) in [
            (Some(4.01), ErrorCode::Invalid),
            (Some(0.99), ErrorCode::Invalid),
            (None, ErrorCode::ModelMissing),
        ] {
            let Response::Error(error) = api.dispatch(request(&path, &id, scale)) else {
                panic!("expected failure")
            };
            assert_eq!(error.code, code);
            if code == ErrorCode::ModelMissing {
                for text in ["realesr-general-x4v3", "4866417 bytes", "Preview"] {
                    assert!(error.message.contains(text), "{error}");
                }
            }
        }
        let status = api.ai_status();
        assert_eq!(status["videoEnhancement"]["installed"], false);
        assert_eq!(
            status["videoEnhancement"]["models"][0]["sizeBytes"],
            4_866_417
        );
        assert!(!enhance::local_model(&api.dirs.data).2.exists());
        assert_eq!(api.running(), 0);
    }

    struct Window(Mutex<Session>);

    impl Live for Window {
        fn path(&self) -> Option<String> {
            Some(self.0.lock().unwrap().path().to_owned())
        }
        fn snapshot(&self) -> Result<Session, String> {
            let session = self.0.lock().unwrap();
            Session::detached(session.path(), session.settings(), &session.document())
        }
        fn apply(&self, command: Command) -> Result<concat_host::session::EditorView, String> {
            self.0.lock().unwrap().apply(command)
        }
        fn undo(&self) -> Result<concat_host::session::EditorView, String> {
            unreachable!()
        }
        fn redo(&self) -> Result<concat_host::session::EditorView, String> {
            unreachable!()
        }
        fn save(&self) -> Result<(), String> {
            unreachable!()
        }
    }

    #[test]
    fn a_cached_copy_replaces_the_clip_in_headless_and_live_projects() {
        for live in [false, true] {
            let (mut api, _scratch, path, id, source) = setup(Arc::new(|_| {}));
            let window = if live {
                let session = api.session(&path).unwrap();
                let window = Arc::new(Window(Mutex::new(
                    Session::detached(session.path(), session.settings(), &session.document())
                        .unwrap(),
                )));
                api.set_live(window.clone());
                Some(window)
            } else {
                None
            };
            let (model, _, dir) = enhance::local_model(&api.dirs.data);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join(format!("{model}.onnx")),
                b"installation marker only",
            )
            .unwrap();
            let cache = enhance::target_for(
                Path::new(&path),
                source.to_str().unwrap(),
                4.0,
                enhance::DEFAULT_STRENGTH,
                true,
                None,
            )
            .unwrap();
            fs::create_dir_all(cache.parent().unwrap()).unwrap();
            png(&cache);
            assert_eq!(api.ai_status()["videoEnhancement"]["installed"], true);
            let Response::Result(Reply::Started(job)) =
                api.dispatch(request(&path, &id, Some(4.0)))
            else {
                panic!("start cached enhancement")
            };
            assert_eq!(Path::new(&job.output), cache);
            api.finish();
            assert_eq!(api.job_status(&job.job).unwrap().state, JobState::Done);
            let snapshot = window.as_ref().map(|window| window.snapshot().unwrap());
            let project = snapshot
                .as_ref()
                .unwrap_or_else(|| api.session(&path).unwrap())
                .project();
            let clip = project.active().clip(&id).unwrap();
            assert!(clip.enhanced.as_ref().unwrap().on);
            let selected = project
                .media
                .iter()
                .find(|item| item.id == clip.media_id)
                .unwrap();
            assert_eq!(Path::new(&selected.path), cache);
            assert!(source.is_file());
        }
    }

    #[test]
    fn progress_and_cancel_use_the_existing_job_protocol() {
        let (sent, received) = mpsc::channel();
        let (mut api, _scratch, path, id, source) = setup(Arc::new(move |event| {
            sent.send(event).unwrap();
        }));
        let project = api.session(&path).unwrap().project();
        let edit = Edit {
            timeline: project.active_timeline_id.clone(),
            clip: project.active().clip(&id).unwrap().clone(),
            source: source.to_string_lossy().into_owned(),
            name: "fixture".to_owned(),
            start: 0.0,
            offset: 0.0,
        };
        let output = Path::new(&path).join("cache/copy.png");
        let (resume, wait) = mpsc::channel();
        let job = api
            .start_enhancement(
                &path,
                output.clone(),
                edit,
                None,
                move |_, cancel, report| {
                    report(0.5);
                    wait.recv_timeout(Duration::from_secs(5)).unwrap();
                    assert!(cancel.load(Ordering::Relaxed));
                    Ok(())
                },
            )
            .unwrap();
        assert!(matches!(
            received.recv_timeout(Duration::from_secs(5)),
            Ok(Event::EnhanceProgress { fraction: 0.5, .. })
        ));
        let status = api.job_status(&job.job).unwrap();
        assert_eq!(status.state, JobState::Running);
        assert_eq!(status.fraction, 0.5);
        assert_eq!(api.ai_status()["videoEnhancement"]["busy"], true);
        api.cancel(&job.job).unwrap();
        resume.send(()).unwrap();
        api.finish();
        assert_eq!(api.job_status(&job.job).unwrap().state, JobState::Cancelled);
        assert!(
            matches!(received.recv_timeout(Duration::from_secs(5)), Ok(Event::EnhanceFailed { error, .. }) if error.code == ErrorCode::Cancelled)
        );
        assert!(!output.exists());
        assert!(
            api.session(&path)
                .unwrap()
                .project()
                .active()
                .clip(&id)
                .unwrap()
                .enhanced
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_cache_symlinks_before_creating_job() {
        let (mut api, _scratch, path, id, _) = setup(Arc::new(|_| {}));
        let (model, _, dir) = enhance::local_model(&api.dirs.data);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{model}.onnx")), b"installation marker").unwrap();
        let outside = tempfile::tempdir().unwrap();
        let cache = Path::new(&path).join("cache");
        if cache.exists() {
            fs::remove_dir(&cache).unwrap();
        }
        std::os::unix::fs::symlink(outside.path(), &cache).unwrap();
        let Response::Error(error) = api.dispatch(request(&path, &id, None)) else {
            panic!("expected refusal")
        };
        assert_eq!(error.code, ErrorCode::Refused);
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        assert_eq!(api.running(), 0);
    }
}
