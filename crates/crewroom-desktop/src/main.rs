mod accessibility;
mod agent_state;
mod client;
mod wall;
mod workbench;
use gpui_kit::{
    AppContext, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions,
    application, assets, component, init, open_window, px, size,
};
use std::path::PathBuf;
actions!(crewroom, [Quit]);
const QUIT_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "cmd-q"
} else {
    "ctrl-q"
};

#[cfg(test)]
const SELECT_ALL_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "cmd-a"
} else {
    "ctrl-a"
};

fn main() {
    let observer: Vec<_> = std::env::args().collect();
    if observer.get(1).is_some_and(|v| v == "--observe-terminal") {
        let result = if observer.len() == 5 {
            wall::observe(
                std::path::Path::new(&observer[2]),
                &observer[3],
                &observer[4],
            )
        } else {
            Err(anyhow::anyhow!("Invalid terminal observer arguments."))
        };
        if let Err(error) = result {
            eprintln!("Terminal unavailable: {error}");
            std::process::exit(1);
        }
        return;
    }
    let mut args = std::env::args().skip(1);
    let mut descriptor = PathBuf::from(
        std::env::var("CREWROOM_NATIVE_DATA_DIR").unwrap_or_else(|_| ".crewroom/rust".into()),
    )
    .join("native-connection.json");
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--connection" => descriptor = args.next().expect("--connection needs a path").into(),
            "--help" => {
                println!("crewroom [--connection /absolute/path/to/native-connection.json]");
                return;
            }
            _ => {
                eprintln!("Unknown argument: {arg}");
                std::process::exit(2);
            }
        }
    }
    application().with_assets(assets::Assets).run(move |cx| {
        init(cx);
        component::Theme::change(component::ThemeMode::Dark, None, cx);
        cx.bind_keys([KeyBinding::new(QUIT_SHORTCUT, Quit, None)]);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1280.), px(880.)),
                cx,
            ))),
            window_min_size: Some(size(px(390.), px(640.))),
            titlebar: Some(TitlebarOptions {
                title: Some("Crewroom".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let (main_window, main_view) = open_window(options, cx, move |window, cx| {
            let (client, events) = client::Client::connect(descriptor);
            let view =
                cx.new(|cx| workbench::Workbench::new(Some(client), Some(events), window, cx));
            let closing = view.clone();
            window.on_window_should_close(cx, move |window, cx| {
                closing.update(cx, |view, cx| view.request_close(window, cx))
            });
            view
        })
        .expect("Open Crewroom window");
        accessibility::install_native_focus_bridge();
        // Hidden input entities can retain a focus handle after their dispatch
        // nodes disappear. Global Quit must still consult the workbench drafts.
        cx.on_action(move |_: &Quit, cx| {
            let _ = main_window.update(cx, |_, window, cx| {
                window.activate_window();
                main_view.update(cx, |view, cx| {
                    view.request_close(window, cx);
                });
            });
        });
        cx.activate(true);
    });
}
