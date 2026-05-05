use std::{
    borrow::Cow,
    env,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

use anyhow::{Context, Result};
use winsafe::{self as w, co, gui, prelude::*};

use crate::session::FontSession;

use super::{
    commands,
    config::GuiConfig,
    state::{AppState, IndexStatus, LoadStatus},
    worker::{self, GuiEvent, GuiTask},
};

pub fn run() -> Result<()> {
    let exe_path = env::current_exe().context("failed to locate current executable")?;
    let exe_dir = exe_path
        .parent()
        .context("current executable has no parent directory")?
        .to_path_buf();
    let config_path = exe_dir.join(commands::CONFIG_FILE_NAME);
    let config = GuiConfig::load(&config_path)?;

    if !config_path.exists() {
        config.save(&config_path)?;
    }

    let startup_inputs = env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let _com_guard =
        w::CoInitializeEx(co::COINIT::APARTMENTTHREADED | co::COINIT::DISABLE_OLE1DDE)?;
    let state = Arc::new(Mutex::new(AppState::new(config, config_path, exe_dir)));

    MainWindow::create_and_run(state, startup_inputs).map_err(|error| anyhow::anyhow!(error))?;
    Ok(())
}

#[derive(Clone)]
struct MainWindow {
    wnd: gui::WindowMain,
    font_root_edit: gui::Edit,
    change_dir_btn: gui::Button,
    update_btn: gui::Button,
    load_btn: gui::Button,
    result_edit: gui::Edit,
    status_edit: gui::Edit,
}

impl MainWindow {
    fn create_and_run(
        state: Arc<Mutex<AppState>>,
        startup_inputs: Vec<PathBuf>,
    ) -> w::AnyResult<i32> {
        let wnd = gui::WindowMain::new(gui::WindowMainOpts {
            title: commands::APP_TITLE,
            size: gui::dpi(900, 650),
            style: co::WS::CAPTION
                | co::WS::SYSMENU
                | co::WS::CLIPCHILDREN
                | co::WS::BORDER
                | co::WS::VISIBLE
                | co::WS::SIZEBOX
                | co::WS::MINIMIZEBOX
                | co::WS::MAXIMIZEBOX,
            ex_style: co::WS_EX::LEFT | co::WS_EX::ACCEPTFILES,
            ..Default::default()
        });

        let font_root_edit = gui::Edit::new(
            &wnd,
            gui::EditOpts {
                position: gui::dpi(12, 12),
                width: gui::dpi_x(560),
                height: gui::dpi_y(24),
                control_style: co::ES::AUTOHSCROLL | co::ES::READONLY | co::ES::NOHIDESEL,
                resize_behavior: (gui::Horz::Resize, gui::Vert::None),
                ..Default::default()
            },
        );
        let change_dir_btn = button(&wnd, commands::BTN_CHANGE_FONT_DIR, 585, 10, 160);
        let update_btn = button(&wnd, commands::BTN_UPDATE_INDEX, 12, 44, 120);
        let load_btn = button(&wnd, commands::BTN_LOAD_SUBTITLES, 144, 44, 120);
        let result_edit = gui::Edit::new(
            &wnd,
            gui::EditOpts {
                position: gui::dpi(12, 82),
                width: gui::dpi_x(860),
                height: gui::dpi_y(455),
                control_style: co::ES::MULTILINE
                    | co::ES::AUTOVSCROLL
                    | co::ES::AUTOHSCROLL
                    | co::ES::READONLY
                    | co::ES::WANTRETURN
                    | co::ES::NOHIDESEL,
                window_style: co::WS::CHILD
                    | co::WS::GROUP
                    | co::WS::TABSTOP
                    | co::WS::VISIBLE
                    | co::WS::VSCROLL
                    | co::WS::HSCROLL,
                resize_behavior: (gui::Horz::Resize, gui::Vert::Resize),
                ..Default::default()
            },
        );
        let status_edit = gui::Edit::new(
            &wnd,
            gui::EditOpts {
                position: gui::dpi(12, 550),
                width: gui::dpi_x(860),
                height: gui::dpi_y(70),
                control_style: co::ES::MULTILINE
                    | co::ES::AUTOVSCROLL
                    | co::ES::READONLY
                    | co::ES::NOHIDESEL,
                window_style: co::WS::CHILD
                    | co::WS::GROUP
                    | co::WS::TABSTOP
                    | co::WS::VISIBLE
                    | co::WS::VSCROLL,
                resize_behavior: (gui::Horz::Resize, gui::Vert::Repos),
                ..Default::default()
            },
        );

        let app = Self {
            wnd,
            font_root_edit,
            change_dir_btn,
            update_btn,
            load_btn,
            result_edit,
            status_edit,
        };

        app.events(state, startup_inputs);
        app.wnd.run_main(None)
    }

    fn events(&self, state: Arc<Mutex<AppState>>, startup_inputs: Vec<PathBuf>) {
        let app = self.clone();
        let state_for_create = Arc::clone(&state);
        self.wnd.on().wm_create(move |_| {
            app.initialize_after_create(Arc::clone(&state_for_create), startup_inputs.clone())?;
            Ok(0)
        });

        let app = self.clone();
        let state_for_change = Arc::clone(&state);
        self.change_dir_btn.on().bn_clicked(move || {
            if let Some(folder) = app.choose_folder("Choose Font Directory")? {
                app.change_font_root(Arc::clone(&state_for_change), folder)?;
            }
            Ok(())
        });

        let app = self.clone();
        let state_for_update = Arc::clone(&state);
        self.update_btn.on().bn_clicked(move || {
            app.start_index_task(
                Arc::clone(&state_for_update),
                IndexStatus::Updating,
                make_update_task(&state_for_update),
                None,
            );
            Ok(())
        });

        let app = self.clone();
        let state_for_load = Arc::clone(&state);
        self.load_btn.on().bn_clicked(move || {
            if state_for_load.lock().unwrap().has_active_load() {
                app.start_unload(Arc::clone(&state_for_load));
            } else if let Some(paths) = app.choose_subtitle_inputs()? {
                app.start_load_inputs(Arc::clone(&state_for_load), paths);
            }
            Ok(())
        });

        let app = self.clone();
        let state_for_drop = Arc::clone(&state);
        self.wnd.on().wm_drop_files(move |params| {
            let reject_drop = {
                let state = state_for_drop.lock().unwrap();
                state.is_busy || state.has_active_load()
            };
            if reject_drop {
                return Ok(());
            }

            let dropped_paths = params
                .hdrop
                .DragQueryFile()?
                .collect::<w::SysResult<Vec<_>>>()?
                .into_iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>();
            app.start_load_inputs(Arc::clone(&state_for_drop), dropped_paths);
            Ok(())
        });
    }

    fn initialize_after_create(
        &self,
        state: Arc<Mutex<AppState>>,
        startup_inputs: Vec<PathBuf>,
    ) -> w::AnyResult<()> {
        self.wnd.hwnd().DragAcceptFiles(true);
        self.set_report_text("")?;

        let auto_index = {
            let mut state = state.lock().unwrap();
            if state.config.auto_index_on_startup {
                state.index_status = IndexStatus::Building;
                true
            } else {
                state.index_status =
                    match worker::inspect_index_status(&state.font_root, &state.db_path) {
                        Ok(inspection) if state.db_path.exists() => {
                            IndexStatus::from_inspection(inspection)
                        }
                        Ok(_) => IndexStatus::DisabledByConfig,
                        Err(error) => IndexStatus::Failed(format!("{error:#}")),
                    };
                false
            }
        };
        self.update_ui(&state.lock().unwrap());

        if auto_index {
            self.start_index_task(
                Arc::clone(&state),
                IndexStatus::Building,
                make_startup_index_task(&state),
                Some(startup_inputs),
            );
        } else if should_auto_load_startup(&state, &startup_inputs) {
            self.start_load_inputs(state, startup_inputs);
        }

        Ok(())
    }

    fn change_font_root(&self, state: Arc<Mutex<AppState>>, folder: PathBuf) -> w::AnyResult<()> {
        let task = {
            let mut state = state.lock().unwrap();
            let new_root = folder.canonicalize().unwrap_or(folder);
            let old_root = state
                .font_root
                .canonicalize()
                .unwrap_or_else(|_| state.font_root.clone());

            if paths_equal(&old_root, &new_root) {
                return Ok(());
            }

            state.font_root = new_root.clone();
            state.config.font_root = new_root.to_string_lossy().into_owned();
            state.config.save(&state.config_path)?;
            state.last_view = None;
            state.last_subtitle_inputs.clear();
            state.index_status = IndexStatus::Building;
            state.load_status = LoadStatus::Idle;
            state.is_busy = true;
            let _ = self.set_report_text("");

            GuiTask::SwitchFontRoot {
                old_session: state.font_session.take().unwrap_or_default(),
                font_root: new_root,
                db_path: state.db_path.clone(),
            }
        };

        self.update_ui(&state.lock().unwrap());
        self.spawn_worker(state, task, None);
        Ok(())
    }

    fn start_index_task(
        &self,
        state: Arc<Mutex<AppState>>,
        status: IndexStatus,
        task: GuiTask,
        startup_inputs: Option<Vec<PathBuf>>,
    ) {
        {
            let mut state = state.lock().unwrap();
            state.index_status = status;
            state.is_busy = true;
            self.update_ui(&state);
        }

        self.spawn_worker(state, task, startup_inputs);
    }

    fn start_load_inputs(&self, state: Arc<Mutex<AppState>>, inputs: Vec<PathBuf>) {
        if inputs.is_empty() {
            return;
        }

        let task = {
            let mut state = state.lock().unwrap();
            if state.is_busy {
                self.show_message("Another operation is already running.");
                return;
            }

            if state.has_active_load() {
                self.show_message("Unload the current fonts before loading another subtitle.");
                return;
            }

            if !state.index_status.is_ready() {
                let message = state.index_status.load_block_message();
                state.load_status = LoadStatus::Failed(message.clone());
                self.update_ui(&state);
                self.show_message(&message);
                return;
            }

            state.last_subtitle_inputs = inputs.clone();
            state.load_status = LoadStatus::AnalyzingSubtitles;
            state.is_busy = true;
            self.update_ui(&state);

            GuiTask::LoadSubtitleInputs {
                inputs,
                font_root: state.font_root.clone(),
                db_path: state.db_path.clone(),
                avoid_system_fonts: state.config.avoid_system_fonts,
                current_session: state.font_session.take().unwrap_or_default(),
            }
        };

        self.spawn_worker(state, task, None);
    }

    fn start_unload(&self, state: Arc<Mutex<AppState>>) {
        let task = {
            let mut state = state.lock().unwrap();
            if state.is_busy || !state.has_active_load() {
                return;
            }

            state.is_busy = true;
            state.load_status = LoadStatus::Idle;
            self.update_ui(&state);

            GuiTask::UnloadFonts {
                session: state.font_session.take().unwrap_or_default(),
            }
        };

        self.spawn_worker(state, task, None);
    }

    fn spawn_worker(
        &self,
        state: Arc<Mutex<AppState>>,
        task: GuiTask,
        startup_inputs: Option<Vec<PathBuf>>,
    ) {
        let dispatcher = self.wnd.clone();
        let app = self.clone();

        thread::spawn(move || {
            let event = worker::run_task(task);
            dispatcher.run_ui_thread(move || {
                app.apply_event(Arc::clone(&state), event, startup_inputs)?;
                Ok(())
            });
        });
    }

    fn apply_event(
        &self,
        state: Arc<Mutex<AppState>>,
        event: GuiEvent,
        startup_inputs: Option<Vec<PathBuf>>,
    ) -> w::AnyResult<()> {
        let mut auto_load = None;

        {
            let mut state = state.lock().unwrap();
            state.is_busy = false;

            match event {
                GuiEvent::IndexReady { summary } => {
                    state.index_status = IndexStatus::from_summary(&summary);
                    state.load_status = LoadStatus::Idle;

                    if let Some(inputs) = startup_inputs
                        && state.config.auto_load_startup_subtitles
                        && !inputs.is_empty()
                    {
                        auto_load = Some(inputs);
                    }
                }
                GuiEvent::IndexFailed { error } => {
                    state.index_status = IndexStatus::Failed(error);
                }
                GuiEvent::IndexUnavailable { inspection } => {
                    state.index_status = IndexStatus::from_inspection(inspection);
                }
                GuiEvent::FontRootSwitched {
                    summary,
                    session,
                    unloaded_count,
                } => {
                    state.font_session = Some(session);
                    state.index_status = IndexStatus::from_summary(&summary);
                    state.load_status = LoadStatus::Idle;
                    let _ = self.set_report_text(&format!(
                        "Font directory switched.\r\nUnloaded fonts: {unloaded_count}"
                    ));
                }
                GuiEvent::FontsLoaded { view, session } => {
                    let rendered = view.render_text();
                    state.font_session = Some(session);
                    state.last_view = Some(view.clone());
                    state.load_status = LoadStatus::Loaded(view);
                    let _ = self.set_report_text(&rendered);
                }
                GuiEvent::FontsUnloaded {
                    session,
                    unloaded_count,
                } => {
                    state.font_session = Some(session);
                    state.load_status = LoadStatus::Idle;
                    let _ = self.set_report_text(&format!("Unloaded fonts: {unloaded_count}"));
                }
                GuiEvent::Error(error) => {
                    if matches!(
                        state.index_status,
                        IndexStatus::Building | IndexStatus::Updating
                    ) {
                        state.index_status = IndexStatus::Failed(error.clone());
                    }
                    state.load_status = LoadStatus::Failed(error.clone());
                    state.font_session.get_or_insert_with(FontSession::new);
                    self.show_message(&error);
                }
            }

            self.update_ui(&state);
        }

        if let Some(inputs) = auto_load {
            self.start_load_inputs(state, inputs);
        }

        Ok(())
    }

    fn update_ui(&self, state: &AppState) {
        let _ = self
            .font_root_edit
            .set_text(&state.font_root.to_string_lossy());
        let _ = self.status_edit.set_text(&format!(
            "{}\r\n{}\r\nLoaded session fonts: {}\r\nExecutable directory: {}\r\nConfig: auto_index_on_startup={}, auto_load_startup_subtitles={}, avoid_system_fonts={}",
            state.index_status.status_text(),
            state.load_status.status_text(),
            state.loaded_font_count(),
            state.exe_dir.display(),
            state.config.auto_index_on_startup,
            state.config.auto_load_startup_subtitles,
            state.config.avoid_system_fonts,
        ));

        let idle = !state.is_busy;
        let has_active_load = state.has_active_load();
        let load_button_text = if has_active_load {
            commands::BTN_UNLOAD_FONTS
        } else {
            commands::BTN_LOAD_SUBTITLES
        };
        let _ = self.load_btn.hwnd().SetWindowText(load_button_text);
        self.wnd.hwnd().DragAcceptFiles(idle && !has_active_load);
        self.change_dir_btn.hwnd().EnableWindow(idle);
        self.update_btn.hwnd().EnableWindow(idle);
        self.load_btn
            .hwnd()
            .EnableWindow(idle && (has_active_load || state.index_status.is_ready()));
    }

    fn set_report_text(&self, text: &str) -> w::SysResult<()> {
        let text = edit_multiline_text(text);
        self.result_edit.set_text(&text)
    }

    fn choose_subtitle_inputs(&self) -> w::AnyResult<Option<Vec<PathBuf>>> {
        let answer = self.wnd.hwnd().MessageBox(
            "Choose subtitle files?\r\nClick No to choose a subtitle directory.",
            commands::APP_TITLE,
            co::MB::YESNOCANCEL | co::MB::ICONQUESTION,
        )?;

        if answer == co::DLGID::YES {
            self.choose_subtitle_files().map(Some)
        } else if answer == co::DLGID::NO {
            self.choose_folder("Choose Subtitle Directory")
                .map(|value| value.map(|path| vec![path]))
        } else {
            Ok(None)
        }
    }

    fn choose_subtitle_files(&self) -> w::AnyResult<Vec<PathBuf>> {
        let dialog = create_open_dialog()?;
        dialog.SetOptions(
            dialog.GetOptions()?
                | co::FOS::FORCEFILESYSTEM
                | co::FOS::FILEMUSTEXIST
                | co::FOS::PATHMUSTEXIST
                | co::FOS::ALLOWMULTISELECT
                | co::FOS::NOCHANGEDIR,
        )?;
        dialog.SetTitle("Choose Subtitle Files")?;
        dialog.SetFileTypes(&[("Subtitle files", "*.ass;*.ssa"), ("All files", "*.*")])?;
        dialog.SetFileTypeIndex(1)?;

        if !dialog.Show(self.wnd.hwnd())? {
            return Ok(Vec::new());
        }

        let mut paths = Vec::new();
        for item in dialog.GetResults()?.iter()? {
            paths.push(PathBuf::from(item?.GetDisplayName(co::SIGDN::FILESYSPATH)?));
        }
        Ok(paths)
    }

    fn choose_folder(&self, title: &str) -> w::AnyResult<Option<PathBuf>> {
        let dialog = create_open_dialog()?;
        dialog.SetOptions(
            dialog.GetOptions()?
                | co::FOS::FORCEFILESYSTEM
                | co::FOS::PICKFOLDERS
                | co::FOS::PATHMUSTEXIST
                | co::FOS::NOCHANGEDIR,
        )?;
        dialog.SetTitle(title)?;

        if !dialog.Show(self.wnd.hwnd())? {
            return Ok(None);
        }

        Ok(Some(PathBuf::from(
            dialog.GetResult()?.GetDisplayName(co::SIGDN::FILESYSPATH)?,
        )))
    }

    fn show_message(&self, message: &str) {
        let _ = self.wnd.hwnd().MessageBox(
            message,
            commands::APP_TITLE,
            co::MB::OK | co::MB::ICONINFORMATION,
        );
    }
}

fn button(wnd: &gui::WindowMain, text: &'static str, x: i32, y: i32, width: i32) -> gui::Button {
    gui::Button::new(
        wnd,
        gui::ButtonOpts {
            text,
            position: gui::dpi(x, y),
            width: gui::dpi_x(width),
            height: gui::dpi_y(26),
            resize_behavior: (gui::Horz::None, gui::Vert::None),
            ..Default::default()
        },
    )
}

fn edit_multiline_text(text: &str) -> Cow<'_, str> {
    if !needs_edit_line_ending_normalization(text) {
        return Cow::Borrowed(text);
    }

    let mut normalized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                normalized.push_str("\r\n");
                if matches!(chars.peek(), Some('\n')) {
                    chars.next();
                }
            }
            '\n' => normalized.push_str("\r\n"),
            _ => normalized.push(ch),
        }
    }

    Cow::Owned(normalized)
}

fn needs_edit_line_ending_normalization(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().any(|(index, byte)| match byte {
        b'\n' => index == 0 || bytes[index - 1] != b'\r',
        b'\r' => bytes.get(index + 1) != Some(&b'\n'),
        _ => false,
    })
}

fn create_open_dialog() -> w::AnyResult<w::IFileOpenDialog> {
    Ok(w::CoCreateInstance::<w::IFileOpenDialog>(
        &co::CLSID::FileOpenDialog,
        None::<&w::IUnknown>,
        co::CLSCTX::INPROC_SERVER,
    )?)
}

fn make_startup_index_task(state: &Arc<Mutex<AppState>>) -> GuiTask {
    let state = state.lock().unwrap();
    GuiTask::EnsureIndexOnStartup {
        font_root: state.font_root.clone(),
        db_path: state.db_path.clone(),
    }
}

fn make_update_task(state: &Arc<Mutex<AppState>>) -> GuiTask {
    let state = state.lock().unwrap();
    GuiTask::UpdateIndex {
        font_root: state.font_root.clone(),
        db_path: state.db_path.clone(),
    }
}

fn should_auto_load_startup(state: &Arc<Mutex<AppState>>, startup_inputs: &[PathBuf]) -> bool {
    let state = state.lock().unwrap();
    state.config.auto_load_startup_subtitles
        && state.index_status.is_ready()
        && !startup_inputs.is_empty()
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    left == right
        || left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::edit_multiline_text;
    use std::borrow::Cow;

    #[test]
    fn edit_multiline_text_converts_lf_to_crlf() {
        assert_eq!(edit_multiline_text("one\ntwo").as_ref(), "one\r\ntwo");
    }

    #[test]
    fn edit_multiline_text_preserves_existing_crlf() {
        assert!(matches!(
            edit_multiline_text("one\r\ntwo"),
            Cow::Borrowed("one\r\ntwo")
        ));
    }

    #[test]
    fn edit_multiline_text_converts_bare_cr_to_crlf() {
        assert_eq!(edit_multiline_text("one\rtwo").as_ref(), "one\r\ntwo");
    }
}
