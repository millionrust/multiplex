use std::io::{self, IsTerminal as _, Stdout, stdout};
use std::mem::ManuallyDrop;
use std::panic;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::thread;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{TerminalOptions, Viewport};
#[cfg(unix)]
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
#[cfg(unix)]
use signal_hook::iterator::Signals;

use crate::localization::TuiLocale;
use crate::model::{ModelAction, ModelEffect, TuiDiagnostic, TuiModel};
use crate::render::{RenderOptions, render};
use crate::source::{FleetCancellation, FleetLoadError, FleetSource};

const EVENT_QUEUE_CAPACITY: usize = 64;
const INLINE_HEIGHT: u16 = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunOptions {
    pub inline: bool,
    pub no_color: bool,
    pub recording_friendly: bool,
    pub locale: TuiLocale,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            inline: false,
            no_color: std::env::var_os("NO_COLOR").is_some(),
            recording_friendly: false,
            locale: TuiLocale::English,
        }
    }
}

enum AppEvent {
    Input(Event),
    Interrupt,
    Refresh {
        generation: u64,
        result: Result<crate::FleetSnapshot, FleetLoadError>,
    },
    TerminalFailure,
}

pub fn run(source: Arc<dyn FleetSource>, options: RunOptions) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "termirust-tui requires an interactive terminal",
        ));
    }
    let mut session = TerminalSession::enter(options.inline)?;
    if std::env::var_os("TERMIRUST_TUI_INJECT_PANIC_AFTER_INIT").is_some() {
        panic!("injected terminal restoration test");
    }

    let (sender, receiver) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
    spawn_input_thread(sender.clone())?;
    spawn_signal_thread(sender.clone())?;

    let mut model = TuiModel::default();
    let mut refresh_cancellation = FleetCancellation::default();
    start_effect(
        model.reduce(ModelAction::BeginRefresh),
        &source,
        &sender,
        &mut refresh_cancellation,
    )?;
    let render_options = RenderOptions {
        no_color: options.no_color,
        recording_friendly: options.recording_friendly,
        locale: options.locale,
    };

    loop {
        session
            .terminal
            .draw(|frame| render(frame, &model, render_options))?;
        if std::env::var_os("TERMIRUST_TUI_EXIT_AFTER_FIRST_DRAW").is_some() {
            break;
        }
        let app_event = receiver
            .recv()
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "TUI event channel closed"))?;
        let effect = match app_event {
            AppEvent::Input(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                key_action(&model, key)
                    .map(|action| model.reduce(action))
                    .unwrap_or(ModelEffect::None)
            }
            AppEvent::Input(_) => ModelEffect::None,
            AppEvent::Interrupt => ModelEffect::Quit,
            AppEvent::Refresh { generation, result } => match result {
                Ok(snapshot) => model.reduce(ModelAction::RefreshSucceeded {
                    generation,
                    snapshot,
                }),
                Err(error) => model.reduce(ModelAction::RefreshFailed {
                    generation,
                    diagnostic: error.diagnostic,
                    recovery_required: error.recovery_required,
                }),
            },
            AppEvent::TerminalFailure => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "terminal input became unavailable",
                ));
            }
        };
        match effect {
            ModelEffect::Quit => break,
            ModelEffect::CancelRefresh => refresh_cancellation.cancel(),
            ModelEffect::StartRefresh(_) => {
                start_effect(effect, &source, &sender, &mut refresh_cancellation)?
            }
            ModelEffect::None => {}
        }
    }
    refresh_cancellation.cancel();
    Ok(())
}

fn start_effect(
    effect: ModelEffect,
    source: &Arc<dyn FleetSource>,
    sender: &SyncSender<AppEvent>,
    active_cancellation: &mut FleetCancellation,
) -> io::Result<()> {
    let ModelEffect::StartRefresh(generation) = effect else {
        return Ok(());
    };
    active_cancellation.cancel();
    *active_cancellation = FleetCancellation::default();
    let cancellation = active_cancellation.clone();
    let source = Arc::clone(source);
    let sender = sender.clone();
    thread::Builder::new()
        .name("termirust-tui-refresh".into())
        .spawn(move || {
            let result =
                panic::catch_unwind(panic::AssertUnwindSafe(|| source.load(&cancellation)))
                    .unwrap_or(Err(FleetLoadError {
                        diagnostic: TuiDiagnostic {
                            code: "refresh-failed",
                            summary: "Fleet refresh stopped unexpectedly",
                            recovery: "Press r to retry; existing results remain unchanged.",
                        },
                        recovery_required: false,
                    }));
            let _ = sender.send(AppEvent::Refresh { generation, result });
        })
        .map(|_| ())
}

fn spawn_input_thread(sender: SyncSender<AppEvent>) -> io::Result<()> {
    thread::Builder::new()
        .name("termirust-tui-input".into())
        .spawn(move || {
            loop {
                match event::read() {
                    Ok(event) => {
                        if sender.send(AppEvent::Input(event)).is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        let _ = sender.send(AppEvent::TerminalFailure);
                        break;
                    }
                }
            }
        })
        .map(|_| ())
}

#[cfg(unix)]
fn spawn_signal_thread(sender: SyncSender<AppEvent>) -> io::Result<()> {
    let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP])?;
    thread::Builder::new()
        .name("termirust-tui-signals".into())
        .spawn(move || {
            if signals.forever().next().is_some() {
                let _ = sender.send(AppEvent::Interrupt);
            }
        })
        .map(|_| ())
}

#[cfg(not(unix))]
fn spawn_signal_thread(_sender: SyncSender<AppEvent>) -> io::Result<()> {
    Ok(())
}

fn key_action(model: &TuiModel, key: KeyEvent) -> Option<ModelAction> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Some(ModelAction::Quit);
    }
    if model.filter_editing() {
        return match key.code {
            KeyCode::Esc => Some(ModelAction::Escape),
            KeyCode::Enter => Some(ModelAction::FinishFilter),
            KeyCode::Backspace => Some(ModelAction::FilterBackspace),
            KeyCode::Char(character) => Some(ModelAction::FilterCharacter(character)),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Char('q') => Some(ModelAction::Quit),
        KeyCode::Char('j') | KeyCode::Down => Some(ModelAction::Move(1)),
        KeyCode::Char('k') | KeyCode::Up => Some(ModelAction::Move(-1)),
        KeyCode::Left => Some(ModelAction::Collapse),
        KeyCode::Right => Some(ModelAction::Expand),
        KeyCode::Enter => Some(ModelAction::Activate),
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
            Some(ModelAction::FocusPrevious)
        }
        KeyCode::BackTab => Some(ModelAction::FocusPrevious),
        KeyCode::Tab => Some(ModelAction::FocusNext),
        KeyCode::Char('/') => Some(ModelAction::StartFilter),
        KeyCode::Char('i') => Some(ModelAction::ToggleInspector),
        KeyCode::Char('r') => Some(ModelAction::BeginRefresh),
        KeyCode::Char('?') => Some(ModelAction::ToggleHelp),
        KeyCode::Esc => Some(ModelAction::Escape),
        _ => None,
    }
}

struct TerminalSession {
    terminal: ManuallyDrop<Terminal<CrosstermBackend<Stdout>>>,
    restoration: Arc<RestorationState>,
}

impl TerminalSession {
    fn enter(inline: bool) -> io::Result<Self> {
        let restoration = Arc::new(RestorationState::new(inline));
        let previous_hook = panic::take_hook();
        let panic_restoration = Arc::clone(&restoration);
        panic::set_hook(Box::new(move |info| {
            panic_restoration.restore_once();
            previous_hook(info);
        }));

        let initialized = (|| {
            enable_raw_mode()?;
            let mut output = stdout();
            if !inline {
                execute!(output, EnterAlternateScreen)?;
            }
            execute!(output, Hide)?;
            let backend = CrosstermBackend::new(output);
            let terminal = if inline {
                Terminal::with_options(
                    backend,
                    TerminalOptions {
                        viewport: Viewport::Inline(INLINE_HEIGHT),
                    },
                )?
            } else {
                Terminal::new(backend)?
            };
            Ok(terminal)
        })();
        match initialized {
            Ok(terminal) => Ok(Self {
                terminal: ManuallyDrop::new(terminal),
                restoration,
            }),
            Err(error) => {
                restoration.restore_once();
                Err(error)
            }
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if self.restoration.claim() {
            let _ = self.terminal.show_cursor();
            self.restoration.restore_claimed(false);
            // The terminal no longer believes its cursor is hidden, so its destructor is silent.
            unsafe { ManuallyDrop::drop(&mut self.terminal) };
        }
    }
}

struct RestorationState {
    restored: AtomicBool,
    inline: bool,
}

impl RestorationState {
    const fn new(inline: bool) -> Self {
        Self {
            restored: AtomicBool::new(false),
            inline,
        }
    }

    fn restore_once(&self) {
        if !self.claim() {
            return;
        }
        self.restore_claimed(true);
    }

    fn claim(&self) -> bool {
        !self.restored.swap(true, Ordering::AcqRel)
    }

    fn restore_claimed(&self, show_cursor: bool) {
        let _ = disable_raw_mode();
        let mut output = stdout();
        let _ = match (show_cursor, self.inline) {
            (true, true) => execute!(output, Show),
            (true, false) => execute!(output, Show, LeaveAlternateScreen),
            (false, true) => Ok(()),
            (false, false) => execute!(output, LeaveAlternateScreen),
        };
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    struct PanicSource;

    impl FleetSource for PanicSource {
        fn load(
            &self,
            _cancellation: &FleetCancellation,
        ) -> Result<crate::FleetSnapshot, FleetLoadError> {
            panic!("fixture panic");
        }
    }

    #[test]
    fn restoration_claim_is_idempotent() {
        let state = RestorationState::new(false);
        assert!(!state.restored.load(Ordering::Acquire));
        assert!(state.claim());
        assert!(!state.claim());
    }

    #[test]
    fn key_mapping_keeps_filter_text_separate_from_commands() {
        let mut model = TuiModel::default();
        model.reduce(ModelAction::StartFilter);
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(
            key_action(&model, q),
            Some(ModelAction::FilterCharacter('q'))
        );
        let control_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(key_action(&model, control_c), Some(ModelAction::Quit));
    }

    #[test]
    fn refresh_worker_converts_source_panic_to_bounded_failure() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let source: Arc<dyn FleetSource> = Arc::new(PanicSource);
        let mut cancellation = FleetCancellation::default();
        start_effect(
            ModelEffect::StartRefresh(7),
            &source,
            &sender,
            &mut cancellation,
        )
        .unwrap();
        let AppEvent::Refresh { generation, result } =
            receiver.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("expected refresh result");
        };
        assert_eq!(generation, 7);
        assert_eq!(result.unwrap_err().diagnostic.code, "refresh-failed");
    }
}
