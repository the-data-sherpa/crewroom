use super::*;
use gpui::test::TestWindowExt;
use gpui::{AnyWindowHandle, Bounds, WindowBounds, WindowOptions, point, size};
use gpui_kit as gpui;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
type OptionsFactory =
    std::sync::Arc<dyn Fn(&std::path::Path) -> crewroom_service::ServiceOptions + Send + Sync>;
/// Native forms use the real authenticated Rust HTTP/WS service and private files.
pub(super) struct Fixture {
    pub(super) root: tempfile::TempDir,
    info: crewroom_service::ConnectionInfo,
    execution: bool,
    production: bool,
    options: Option<OptionsFactory>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Fixture {
    pub(super) fn start() -> Self {
        Self::seeded(false, |_, _| {})
    }
    pub(super) fn seeded(
        execution: bool,
        seed: impl FnOnce(&crewroom_core::store::Store, &std::path::Path),
    ) -> Self {
        Self::seeded_options(execution, execution, seed)
    }
    pub(super) fn projected(
        seed: impl FnOnce(&crewroom_core::store::Store, &std::path::Path),
    ) -> Self {
        Self::seeded_options(false, true, seed)
    }
    fn seeded_options(
        execution: bool,
        production: bool,
        seed: impl FnOnce(&crewroom_core::store::Store, &std::path::Path),
    ) -> Self {
        Self::seeded_custom(execution, production, seed, None)
    }
    pub(super) fn configured(
        seed: impl FnOnce(&crewroom_core::store::Store, &std::path::Path),
        options: impl Fn(&std::path::Path) -> crewroom_service::ServiceOptions + Send + Sync + 'static,
    ) -> Self {
        Self::seeded_custom(false, true, seed, Some(std::sync::Arc::new(options)))
    }
    fn seeded_custom(
        execution: bool,
        production: bool,
        seed: impl FnOnce(&crewroom_core::store::Store, &std::path::Path),
        options: Option<OptionsFactory>,
    ) -> Self {
        let root = tempfile::Builder::new()
            .prefix("crewroom-native-")
            .tempdir_in("/tmp")
            .unwrap();
        let data = root.path().join("data");
        std::fs::create_dir(&data).unwrap();
        {
            let store = crewroom_core::store::Store::open(&data.join("workflow.sqlite")).unwrap();
            seed(&store, root.path());
        }
        let (info, stop, worker) = service(root.path(), execution, production, options.clone());
        Self {
            root,
            info,
            execution,
            production,
            options,
            stop: Some(stop),
            worker: Some(worker),
        }
    }
    pub(super) fn restart(&mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        self.worker.take().unwrap().join().unwrap();
        let (info, stop, worker) = service(
            self.root.path(),
            self.execution,
            self.production,
            self.options.clone(),
        );
        self.info = info;
        self.stop = Some(stop);
        self.worker = Some(worker);
    }
    pub(super) fn get(&self, path: &str) -> Value {
        reqwest::blocking::Client::new()
            .get(format!("{}/api{path}", self.info.origin))
            .bearer_auth(&self.info.token)
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .unwrap()
    }
    pub(super) fn accept_plan(&self, task_id: &str) {
        let task = self.get("/workspace")["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == task_id)
            .unwrap()
            .clone();
        let (status, source) = self.post(
            &format!("/tasks/{task_id}/plan-source"),
            json!({"revision":task["revision"]}),
        );
        assert_eq!(status, 200, "{source}");
        let (status,plan)=self.post(&format!("/tasks/{task_id}/implementation-plans"),json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":task["revision"],"sourceId":source["id"],"content":{"schemaVersion":1,"goal":"Native fixture","scope":[task["brief"]],"exclusions":[],"assumptions":[],"decisions":[],"workPackages":[{"id":"fixture","name":"Fixture work","dependsOn":[],"outputs":["Evidence"],"criteria":(0..task["criteria"].as_array().unwrap().len()).collect::<Vec<_>>(),"validation":["Native interaction fixture"]}]}}));
        assert_eq!(status, 200, "{plan}");
        let (status,decision)=self.post(&format!("/implementation-plans/{}/decision",plan["id"].as_str().unwrap()),json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":task["revision"],"sourceId":source["id"],"decision":"accept"}));
        assert_eq!(status, 200, "{decision}");
    }
    pub(super) fn descriptor(&self) -> PathBuf {
        self.root.path().join("data/native-connection.json")
    }
    pub(super) fn post(&self, path: &str, value: Value) -> (u16, Value) {
        let response = reqwest::blocking::Client::new()
            .post(format!("{}/api{path}", self.info.origin))
            .bearer_auth(&self.info.token)
            .header("Origin", &self.info.origin)
            .json(&value)
            .send()
            .unwrap();
        (response.status().as_u16(), response.json().unwrap())
    }
}
fn service(
    root: &std::path::Path,
    execution: bool,
    production: bool,
    options: Option<OptionsFactory>,
) -> (
    crewroom_service::ConnectionInfo,
    tokio::sync::oneshot::Sender<()>,
    std::thread::JoinHandle<()>,
) {
    let root = std::fs::canonicalize(root).unwrap();
    let data = root.join("data");
    let roles = root.join("roles.json");
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let (send, ready) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async move {
            let service=if let Some(options) = options {
                crewroom_service::Service::start_with_options(&data, &roles, 0, options(&root)).await.unwrap()
            } else if production {
                let hosts=root.join("hosts");
                std::fs::create_dir_all(&hosts).unwrap();
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&hosts,std::fs::Permissions::from_mode(0o700)).unwrap();
                let runtime=json!({"root":hosts,"socket":hosts.join("tmux.sock"),"ompBinary":"/usr/bin/false", "mail":{"url":"http://127.0.0.1:9/mcp/","token":"synthetic-native-fixture","projectRoot":root,"retentionSeconds":86400,"atcDisabled":true}, "catalog":[{"provider":"fixture","id":"model","api":"openai-responses","inputs":["text"],"efforts":["low"],"tools":true}],"policy":{},"models":{"providers":{}}});
                // Positive queued insertion validates real model policy and workflow state.
                // No launch is saved; all executable paths fail closed if accidentally invoked.
                crewroom_service::Service::start_with_options(&data,&roles,0,crewroom_service::ServiceOptions {runtime:execution.then_some(runtime),helper:"/usr/bin/false".into(),host:"/usr/bin/false".into(),tmux:"/usr/bin/false".into(),jira:None,publication:vec![],publication_unavailable:vec![],environment:json!({}),github:None}).await.unwrap()
            } else {crewroom_service::Service::start(&data,&roles,0).await.unwrap()};
            send.send(service.info.clone()).unwrap();let _=stopped.await;service.close().await.unwrap();
        });
    });
    (
        ready.recv_timeout(Duration::from_secs(10)).unwrap(),
        stop,
        worker,
    )
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.worker.take().unwrap().join().unwrap();
    }
}
pub(super) fn open(
    cx: &mut gpui::TestAppContext,
    fixture: &Fixture,
) -> (
    AnyWindowHandle,
    Entity<Workbench>,
    async_channel::Receiver<Event>,
) {
    open_connection(cx, fixture.descriptor())
}
pub(super) fn open_connection(
    cx: &mut gpui::TestAppContext,
    descriptor: PathBuf,
) -> (
    AnyWindowHandle,
    Entity<Workbench>,
    async_channel::Receiver<Event>,
) {
    let (client, events) = Client::connect(descriptor);
    let (handle, view) = cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(1280.), px(1800.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workbench::new(Some(client), None, window, cx)),
        )
        .unwrap()
    });
    (handle, view, events)
}
#[track_caller]
pub(super) fn wait(
    cx: &mut gpui::TestAppContext,
    handle: AnyWindowHandle,
    view: &Entity<Workbench>,
    events: &async_channel::Receiver<Event>,
    predicate: impl Fn(&Workbench, &App) -> bool,
) {
    wait_for(cx, handle, view, events, Duration::from_secs(10), predicate);
}
#[track_caller]
pub(super) fn wait_for(
    cx: &mut gpui::TestAppContext,
    handle: AnyWindowHandle,
    view: &Entity<Workbench>,
    events: &async_channel::Receiver<Event>,
    timeout: Duration,
    predicate: impl Fn(&Workbench, &App) -> bool,
) {
    let until = Instant::now() + timeout;
    loop {
        // The deterministic GPUI test scheduler forbids OS-thread wakeups. Pump
        // the real HTTP/WS client's events on its thread, using the production reducer.
        while let Ok(event) = events.try_recv() {
            cx.update(|cx| view.update(cx, |v, cx| v.accept(event, cx)));
        }
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        if cx.read(|cx| predicate(view.read(cx), cx)) {
            return;
        }
        assert!(
            Instant::now() < until,
            "Native state timeout: {}",
            cx.read(|cx| format!(
                "{}; queue {}; observation {} / fresh {}:{} / errors {:?}",
                view.read(cx).error,
                view.read(cx).workspace["queue"],
                view.read(cx).observation.id.as_deref().unwrap_or("none"),
                view.read(cx).observation.runtime_fresh,
                view.read(cx).observation.result_fresh,
                view.read(cx).observation.errors
            ))
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
/// Tab traversal also reveals controls inside nested scroll containers.
pub(super) fn activate_by_keyboard(
    cx: &mut gpui::TestAppContext,
    handle: AnyWindowHandle,
    id: impl Into<gpui::ElementId>,
) {
    cx.run_until_parked();
    let id = id.into();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        for _ in 0..200 {
            if window.find(id.clone()).focused() == Some(true) {
                assert!(window.find(id.clone()).visible());
                window.press("enter", cx);
                window.render_frame(cx);
                return;
            }
            window.press("tab", cx);
            window.render_frame(cx);
        }
        panic!("Control {id:?} was not keyboard reachable");
    })
    .unwrap();
}

pub(super) fn click(
    cx: &mut gpui::TestAppContext,
    handle: AnyWindowHandle,
    id: impl Into<gpui::ElementId>,
) {
    cx.run_until_parked();
    let id = id.into();
    // GPUI component dialogs animate with wall time, outside its test clock.
    // Let the 250 ms entry finish before computing a pointer target.
    if id == gpui::ElementId::from("ok") || id == gpui::ElementId::from("cancel") {
        std::thread::sleep(Duration::from_millis(300));
    }
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // Scroll a clipped form control into view before dispatching a pointer event.
        for _ in 0..20 {
            let bounds = window.find(id.clone()).bounds();
            if bounds.origin.y >= px(55.)
                && bounds.bottom() <= window.viewport_size().height - px(70.)
            {
                break;
            }
            let delta = if bounds.origin.y < px(55.) {
                400.
            } else {
                -400.
            };
            use gpui::InputEvent;
            window.dispatch_event(
                gpui::ScrollWheelEvent {
                    position: point(
                        window.viewport_size().width - px(40.),
                        window.viewport_size().height / 2.,
                    ),
                    delta: gpui::ScrollDelta::Pixels(point(px(0.), px(delta))),
                    modifiers: Default::default(),
                    touch_phase: gpui::TouchPhase::Moved,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        }
        window.click(id, cx);
        window.render_frame(cx);
    })
    .unwrap();
}

/// Session-layout navigation: settings sections sit behind the gear, Brief/Results/History open
/// in the details drawer, and "Conversation" returns from the drawer to the session column.
pub(super) fn navigate(cx: &mut gpui::TestAppContext, handle: AnyWindowHandle, target: &str) {
    navigate_with(cx, handle, target, false);
}
pub(super) fn navigate_by_keyboard(
    cx: &mut gpui::TestAppContext,
    handle: AnyWindowHandle,
    target: &str,
) {
    navigate_with(cx, handle, target, true);
}
fn present(cx: &mut gpui::TestAppContext, handle: AnyWindowHandle, id: &str) -> bool {
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(SharedString::from(id.to_owned())).is_some()
    })
    .unwrap()
}
fn act(cx: &mut gpui::TestAppContext, handle: AnyWindowHandle, id: &str, keyboard: bool) {
    if keyboard {
        super::acceptance_tests::keyboard_activate(cx, handle, id);
    } else {
        click(cx, handle, SharedString::from(id.to_owned()));
    }
}
fn navigate_with(
    cx: &mut gpui::TestAppContext,
    handle: AnyWindowHandle,
    target: &str,
    keyboard: bool,
) {
    match target {
        "Conversation" => {
            if present(cx, handle, "hide-details") {
                act(cx, handle, "hide-details", keyboard);
            }
        }
        "Brief" | "Results" | "History" => {
            if !present(cx, handle, target) {
                act(cx, handle, "toggle-details", keyboard);
            }
            act(cx, handle, target, keyboard);
        }
        // The specialist form lives in the session's picker, opened from the agent area.
        "specialist-picker" => {
            if !present(cx, handle, "propose-specialist") {
                act(cx, handle, "open-specialist-picker", keyboard);
            }
        }
        "roles" | "models" | "principles" | "playbooks" | "repositories" | "learning"
        | "import-jira" => {
            if !present(cx, handle, target) {
                act(cx, handle, "settings", keyboard);
            }
            act(cx, handle, target, keyboard);
        }
        _ => act(cx, handle, target, keyboard),
    }
}

pub(super) fn accept_plan(store: &crewroom_core::store::Store, task_id: &str) {
    let task = store.get_task(task_id).unwrap();
    let source = store
        .save_plan_source(
            &store
                .prepare_plan_source(task_id, task["revision"].as_u64().unwrap())
                .unwrap()
                .validate()
                .unwrap(),
        )
        .unwrap();
    let plan=store.save_implementation_plan(task_id,&json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":task["revision"],"sourceId":source["id"],"content":{"schemaVersion":1,"goal":"Exercise the accepted fixture behavior","scope":[task["brief"]],"exclusions":[],"assumptions":[],"decisions":[],"workPackages":[{"id":"fixture","name":"Fixture work","dependsOn":[],"outputs":["Revision-bound evidence"],"criteria":(0..task["criteria"].as_array().unwrap().len()).collect::<Vec<_>>(),"validation":["The surrounding behavior test"]}]}})).unwrap();
    store.decide_implementation_plan(plan["id"].as_str().unwrap(),&json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":task["revision"],"sourceId":source["id"],"decision":"accept"})).unwrap();
}

/// Every readable label in the window, including an open dialog.
pub(super) fn screen_text(cx: &mut gpui::TestAppContext, handle: AnyWindowHandle) -> Vec<String> {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect()
    })
    .unwrap()
}
