use std::io::{self, IsTerminal as _, Stdout, stdout};
use std::mem::ManuallyDrop;
use std::panic;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use rand::RngCore as _;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{TerminalOptions, Viewport as RatatuiViewport};
#[cfg(unix)]
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
#[cfg(unix)]
use signal_hook::iterator::Signals;

use crate::attach::{
    AttachCommand, AttachEvent, AttachEventSink, AttachWorker, AttachedTerminal, TuiAttachState,
    Viewport, endpoint_for_source, spawn_attach_worker,
};
use crate::input::{InputDecision, InteractiveLease, TuiFocus};
use crate::localization::TuiLocale;
use crate::management::{
    LocalManagementExecutor, ManagementEffect, ManagementExecutor, ManagementFailure,
    ManagementIntent, ManagementModel,
};
use crate::model::{ModelAction, ModelEffect, TuiDiagnostic, TuiModel};
use crate::render::{RenderOptions, render, render_attached, render_management};
use crate::source::{FleetCancellation, FleetLoadError, FleetSource};

const EVENT_QUEUE_CAPACITY: usize = 64;
const INLINE_HEIGHT: u16 = 20;
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(50);
const EVENT_DELIVERY_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_BASE: Duration = Duration::from_millis(250);
const RETRY_CAP: Duration = Duration::from_secs(30);
const RETRY_MAX_ELAPSED: Duration = Duration::from_secs(90);
const RETRY_MAX_ATTEMPTS: u8 = 8;

#[derive(Clone, Debug, Default)]
struct AttachRetry {
    attempts: u8,
    started_at: Option<Instant>,
    deadline: Option<Instant>,
}

impl AttachRetry {
    fn schedule(&mut self, now: Instant, random: u64) -> Option<Duration> {
        let started_at = *self.started_at.get_or_insert(now);
        if self.attempts >= RETRY_MAX_ATTEMPTS
            || now.saturating_duration_since(started_at) >= RETRY_MAX_ELAPSED
        {
            self.deadline = None;
            return None;
        }
        let exponent = u32::from(self.attempts).min(7);
        let cap = RETRY_BASE
            .checked_mul(1_u32 << exponent)
            .unwrap_or(RETRY_CAP)
            .min(RETRY_CAP);
        let remaining = RETRY_MAX_ELAPSED.saturating_sub(now.saturating_duration_since(started_at));
        let cap = cap.min(remaining);
        let cap_millis = u64::try_from(cap.as_millis()).unwrap_or(u64::MAX);
        let delay = Duration::from_millis(random % cap_millis.saturating_add(1));
        self.attempts = self.attempts.saturating_add(1);
        self.deadline = Some(now + delay);
        Some(delay)
    }

    fn due(&self, now: Instant) -> bool {
        self.deadline.is_some_and(|deadline| deadline <= now)
    }

    fn take_deadline(&mut self) {
        self.deadline = None;
    }

    fn reset(&mut self) {
        *self = Self::default();
    }

    fn notice(&self, delay: Duration) -> String {
        format!(
            "Reconnect attempt {} of {RETRY_MAX_ATTEMPTS} in {} ms. Press r to try now; Ctrl+Space then Esc cancels.",
            self.attempts,
            delay.as_millis()
        )
    }
}

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
    Attach(AttachEvent),
    ManagementChoices {
        generation: u64,
        result: Result<Vec<crate::LaunchChoice>, ManagementFailure>,
    },
    ManagementCompleted {
        generation: u64,
        result: Result<crate::ManagementResult, ManagementFailure>,
    },
    Deadline,
    TerminalFailure,
}

pub fn run(source: Arc<dyn FleetSource>, options: RunOptions) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "termirust-tui requires an interactive terminal",
        ));
    }
    let (sender, receiver) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
    spawn_signal_thread(sender.clone())?;
    let mut session = TerminalSession::enter(options.inline)?;
    if std::env::var_os("TERMIRUST_TUI_INJECT_PANIC_AFTER_INIT").is_some() {
        panic!("injected terminal restoration test");
    }

    spawn_input_thread(sender.clone())?;

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
    let mut attached: Option<AttachedTerminal> = None;
    let mut attach_worker: Option<AttachWorker> = None;
    let mut attach_generation = 0_u64;
    let mut terminal_notice: Option<String> = None;
    let mut pending_resize: Option<(Viewport, Instant)> = None;
    let mut attach_retry = AttachRetry::default();
    let management_executor: Option<Arc<dyn ManagementExecutor>> = source
        .config_root()
        .and_then(|root| LocalManagementExecutor::new(root).ok())
        .map(|executor| Arc::new(executor) as Arc<dyn ManagementExecutor>);
    let mut management = ManagementModel::default();

    loop {
        session.terminal.draw(|frame| {
            if let Some(attached) = &attached {
                render_attached(frame, attached, terminal_notice.as_deref(), render_options);
            } else {
                render(frame, &model, render_options);
                render_management(frame, &management, render_options);
            }
        })?;
        if std::env::var_os("TERMIRUST_TUI_EXIT_AFTER_FIRST_DRAW").is_some() {
            break;
        }
        let deadline = nearest_deadline(
            attached.as_ref(),
            pending_resize.as_ref(),
            attach_retry.deadline,
            management.deadline(),
        );
        let app_event = receive_event(&receiver, deadline)?;
        let mut retry_attach = false;
        let mut return_to_fleet = false;
        let effect = match app_event {
            AppEvent::Input(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                if let Some(current) = attached.as_mut() {
                    if current.input().pending_paste().is_none()
                        && current.input().focus() == TuiFocus::Terminal
                        && key.code == KeyCode::Char('i')
                        && current.input().lease() == InteractiveLease::ViewOnly
                        && matches!(
                            current.state(),
                            TuiAttachState::LiveReadOnly | TuiAttachState::Replaying
                        )
                    {
                        if attach_worker
                            .as_ref()
                            .is_some_and(|worker| worker.try_send(AttachCommand::RequestLease))
                        {
                            terminal_notice = Some("Requesting the interactive lease...".into());
                        } else {
                            terminal_notice = Some("The attach worker is unavailable.".into());
                        }
                    } else if current.input().pending_paste().is_none()
                        && current.input().focus() == TuiFocus::Terminal
                        && key.code == KeyCode::Char('r')
                        && matches!(
                            current.state(),
                            TuiAttachState::Gap | TuiAttachState::Unavailable
                        )
                    {
                        attach_retry.take_deadline();
                        retry_attach = true;
                    } else {
                        let decision = current.input_mut().handle_key(key, Instant::now());
                        return_to_fleet = apply_input_decision(
                            decision,
                            attach_worker.as_ref(),
                            &mut terminal_notice,
                        );
                    }
                    ModelEffect::None
                } else if management.active() {
                    let effect = management.handle_key(key, Instant::now());
                    apply_management_effect(
                        effect,
                        &mut management,
                        management_executor.as_ref(),
                        &sender,
                    )?;
                    ModelEffect::None
                } else if let Some(effect) = management_shortcut(&model, &mut management, key) {
                    apply_management_effect(
                        effect,
                        &mut management,
                        management_executor.as_ref(),
                        &sender,
                    )?;
                    ModelEffect::None
                } else if key.code == KeyCode::Enter && model.focus() == crate::PaneFocus::Sessions
                {
                    if let Some(selected) = model.selected_session().cloned() {
                        begin_attach(
                            &source,
                            &sender,
                            selected,
                            &mut attached,
                            &mut attach_worker,
                            &mut attach_generation,
                            session.terminal.size()?.into(),
                        )?;
                    }
                    terminal_notice = None;
                    ModelEffect::None
                } else {
                    key_action(&model, key)
                        .map(|action| model.reduce(action))
                        .unwrap_or(ModelEffect::None)
                }
            }
            AppEvent::Input(Event::Paste(value)) => {
                if let Some(current) = attached.as_mut() {
                    let bracketed = current.terminal().bracketed_paste();
                    let decision = current.input_mut().handle_paste(value, bracketed);
                    return_to_fleet = apply_input_decision(
                        decision,
                        attach_worker.as_ref(),
                        &mut terminal_notice,
                    );
                } else if management.active() {
                    management.append_paste(&value);
                }
                ModelEffect::None
            }
            AppEvent::Input(Event::Resize(columns, rows)) => {
                if let Some(current) = attached.as_mut() {
                    let viewport = current.resize(viewport_for_size(columns, rows));
                    pending_resize = Some((viewport, Instant::now() + RESIZE_DEBOUNCE));
                }
                ModelEffect::None
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
            AppEvent::Attach(event) => {
                let successful_batch = matches!(&event, AttachEvent::Batch { .. });
                if let Some(current) = attached.as_mut()
                    && current.apply(event)
                {
                    if successful_batch {
                        attach_retry.reset();
                        terminal_notice = None;
                    }
                    if current.state() == TuiAttachState::Detached {
                        return_to_fleet = true;
                    } else if matches!(
                        current.state(),
                        TuiAttachState::Gap | TuiAttachState::Unavailable | TuiAttachState::Exited
                    ) {
                        attach_worker.take();
                        if current.failure() == Some(crate::AttachFailure::Unavailable) {
                            let delay =
                                attach_retry.schedule(Instant::now(), rand::rngs::OsRng.next_u64());
                            terminal_notice = delay.map(|delay| attach_retry.notice(delay)).or_else(
                                || Some("Automatic reconnect stopped after its bounded limit. Press r to try now or detach.".into()),
                            );
                        }
                    }
                }
                ModelEffect::None
            }
            AppEvent::ManagementChoices { generation, result } => {
                management.launch_choices_loaded(generation, result);
                ModelEffect::None
            }
            AppEvent::ManagementCompleted { generation, result } => {
                let succeeded = result.is_ok();
                management.completed(generation, result, Instant::now());
                if succeeded {
                    model.reduce(ModelAction::BeginRefresh)
                } else {
                    ModelEffect::None
                }
            }
            AppEvent::Deadline => {
                if let Some(current) = attached.as_mut() {
                    let decision = current.input_mut().expire_leader(Instant::now());
                    return_to_fleet = apply_input_decision(
                        decision,
                        attach_worker.as_ref(),
                        &mut terminal_notice,
                    );
                }
                management.expire_undo(Instant::now());
                ModelEffect::None
            }
            AppEvent::TerminalFailure => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "terminal input became unavailable",
                ));
            }
        };
        if pending_resize
            .as_ref()
            .is_some_and(|(_, deadline)| *deadline <= Instant::now())
            && let Some((viewport, _)) = pending_resize.take()
            && let Some(worker) = &attach_worker
            && !worker.try_send(AttachCommand::Resize(viewport))
        {
            terminal_notice = Some("Resize delivery is busy; the next size will retry.".into());
        }
        if attach_retry.due(Instant::now()) {
            attach_retry.take_deadline();
            retry_attach = true;
            terminal_notice = Some("Retrying the read-only attach path now...".into());
        }
        if retry_attach {
            restart_attach(
                &source,
                &sender,
                &mut attached,
                &mut attach_worker,
                &mut attach_generation,
            )?;
            terminal_notice = None;
        }
        if return_to_fleet {
            attach_worker.take();
            attached = None;
            pending_resize = None;
            terminal_notice = None;
            attach_retry.reset();
        }
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
    attach_worker.take();
    Ok(())
}

fn receive_event(
    receiver: &mpsc::Receiver<AppEvent>,
    deadline: Option<Instant>,
) -> io::Result<AppEvent> {
    let result = deadline.map_or_else(
        || receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
        |deadline| receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())),
    );
    match result {
        Ok(event) => Ok(event),
        Err(RecvTimeoutError::Timeout) => Ok(AppEvent::Deadline),
        Err(RecvTimeoutError::Disconnected) => Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "TUI event channel closed",
        )),
    }
}

fn nearest_deadline(
    attached: Option<&AttachedTerminal>,
    resize: Option<&(Viewport, Instant)>,
    retry: Option<Instant>,
    management: Option<Instant>,
) -> Option<Instant> {
    [
        attached.and_then(|attached| attached.input().leader_deadline()),
        resize.map(|(_, deadline)| *deadline),
        retry,
        management,
    ]
    .into_iter()
    .flatten()
    .min()
}

fn management_shortcut(
    model: &TuiModel,
    management: &mut ManagementModel,
    key: KeyEvent,
) -> Option<ManagementEffect> {
    if model.filter_editing() || model.help_visible() || !key.modifiers.is_empty() {
        return None;
    }
    if key.code == KeyCode::Char('n') {
        let (project_id, project_name, group_id) = launch_context(model)?;
        return Some(management.begin_launch(project_id, project_name, group_id));
    }
    if model.focus() != crate::PaneFocus::Sessions {
        return None;
    }
    let session = model.selected_session()?;
    let intent = match key.code {
        KeyCode::Char('e') => ManagementIntent::Rename,
        KeyCode::Char('p') => ManagementIntent::TogglePin,
        KeyCode::Char('m') => ManagementIntent::MarkRead,
        KeyCode::Char('s') => ManagementIntent::Stop,
        KeyCode::Char('a') if session.archived => ManagementIntent::Restore,
        KeyCode::Char('a') => ManagementIntent::Archive,
        _ => return None,
    };
    Some(management.begin_session(intent, session))
}

fn launch_context(model: &TuiModel) -> Option<(String, String, Option<String>)> {
    let snapshot = model.snapshot()?;
    let (project_id, group_id) = match model.selected_scope() {
        crate::ScopeId::Project(project_id) => (project_id.clone(), None),
        crate::ScopeId::Group(group_id) => {
            let project = snapshot
                .projects
                .iter()
                .find(|project| project.groups.iter().any(|group| &group.id == group_id))?;
            (project.id.clone(), Some(group_id.clone()))
        }
        crate::ScopeId::All => {
            let session = model.selected_session()?;
            (session.project_id.clone(), session.group_id.clone())
        }
    };
    let project = snapshot
        .projects
        .iter()
        .find(|project| project.id == project_id)?;
    Some((project_id, project.name.clone(), group_id))
}

fn apply_management_effect(
    effect: ManagementEffect,
    model: &mut ManagementModel,
    executor: Option<&Arc<dyn ManagementExecutor>>,
    sender: &SyncSender<AppEvent>,
) -> io::Result<()> {
    match effect {
        ManagementEffect::None => Ok(()),
        ManagementEffect::Close => {
            model.close();
            Ok(())
        }
        ManagementEffect::LoadLaunchChoices { project_id } => {
            let generation = model.generation();
            let cancellation = model.cancellation();
            let Some(executor) = executor.cloned() else {
                model.launch_choices_loaded(generation, Err(ManagementFailure::unavailable()));
                return Ok(());
            };
            let sender = sender.clone();
            thread::Builder::new()
                .name("termirust-tui-management-read".into())
                .spawn(move || {
                    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                        executor.launch_choices(&project_id, &cancellation)
                    }))
                    .unwrap_or_else(|_| Err(ManagementFailure::unavailable()));
                    let _ = sender.send(AppEvent::ManagementChoices { generation, result });
                })
                .map(|_| ())
        }
        ManagementEffect::Execute(command) => {
            let generation = model.generation();
            let cancellation = model.cancellation();
            let Some(executor) = executor.cloned() else {
                model.completed(
                    generation,
                    Err(ManagementFailure::unavailable()),
                    Instant::now(),
                );
                return Ok(());
            };
            let sender = sender.clone();
            thread::Builder::new()
                .name("termirust-tui-management-command".into())
                .spawn(move || {
                    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                        executor.execute(command, &cancellation)
                    }))
                    .unwrap_or_else(|_| Err(ManagementFailure::unavailable()));
                    let _ = sender.send(AppEvent::ManagementCompleted { generation, result });
                })
                .map(|_| ())
        }
    }
}

fn begin_attach(
    source: &Arc<dyn FleetSource>,
    sender: &SyncSender<AppEvent>,
    selected: crate::FleetSession,
    attached: &mut Option<AttachedTerminal>,
    worker: &mut Option<AttachWorker>,
    generation: &mut u64,
    size: ratatui::layout::Rect,
) -> io::Result<()> {
    let session_id = selected.id.parse().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "selected session identity is invalid",
        )
    })?;
    let endpoint = endpoint_for_source(source.as_ref(), session_id)
        .map_err(|failure| io::Error::new(io::ErrorKind::NotFound, failure.message()))?;
    *generation = generation.saturating_add(1);
    let model = AttachedTerminal::new(
        *generation,
        session_id,
        selected.title,
        viewport_for_size(size.width, size.height),
    );
    let event_sender = sender.clone();
    let sink: AttachEventSink = Arc::new(move |event| enqueue_attach_event(&event_sender, event));
    let next_worker = spawn_attach_worker(
        model.generation(),
        endpoint,
        model.session_id(),
        model.viewport(),
        sink,
    )?;
    worker.replace(next_worker);
    attached.replace(model);
    Ok(())
}

fn restart_attach(
    source: &Arc<dyn FleetSource>,
    sender: &SyncSender<AppEvent>,
    attached: &mut Option<AttachedTerminal>,
    worker: &mut Option<AttachWorker>,
    generation: &mut u64,
) -> io::Result<()> {
    let Some(previous) = attached.take() else {
        return Ok(());
    };
    worker.take();
    let session_id = previous.session_id();
    let endpoint = endpoint_for_source(source.as_ref(), session_id)
        .map_err(|failure| io::Error::new(io::ErrorKind::NotFound, failure.message()))?;
    *generation = generation.saturating_add(1);
    let model = AttachedTerminal::new(
        *generation,
        session_id,
        previous.title().to_string(),
        previous.viewport(),
    );
    let event_sender = sender.clone();
    let sink: AttachEventSink = Arc::new(move |event| enqueue_attach_event(&event_sender, event));
    let next_worker = spawn_attach_worker(
        model.generation(),
        endpoint,
        session_id,
        model.viewport(),
        sink,
    )?;
    worker.replace(next_worker);
    attached.replace(model);
    Ok(())
}

fn viewport_for_size(columns: u16, rows: u16) -> Viewport {
    Viewport::new(columns.max(1), rows.saturating_sub(5).max(1))
}

fn enqueue_attach_event(sender: &SyncSender<AppEvent>, event: AttachEvent) -> bool {
    let deadline = Instant::now() + EVENT_DELIVERY_TIMEOUT;
    let mut event = event;
    loop {
        match sender.try_send(AppEvent::Attach(event)) {
            Ok(()) => return true,
            Err(TrySendError::Full(AppEvent::Attach(pending))) if Instant::now() < deadline => {
                event = pending;
                thread::sleep(Duration::from_millis(1));
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

fn apply_input_decision(
    decision: InputDecision,
    worker: Option<&AttachWorker>,
    notice: &mut Option<String>,
) -> bool {
    match decision {
        InputDecision::None => false,
        InputDecision::Send(bytes) => {
            if !worker.is_some_and(|worker| worker.try_send(AttachCommand::Input(bytes))) {
                *notice = Some("Input was not queued because the Host is unavailable.".into());
            }
            false
        }
        InputDecision::Detach => {
            if !worker.is_some_and(|worker| worker.try_send(AttachCommand::Detach)) {
                *notice = Some("The client disconnected; the Host was not stopped.".into());
                return true;
            }
            *notice = Some("Detaching from the Host...".into());
            false
        }
        InputDecision::ConfirmPaste { bytes, multiline } => {
            *notice = Some(format!(
                "Confirm {bytes}-byte{} paste with Enter or cancel with Esc.",
                if multiline { " multiline" } else { "" }
            ));
            false
        }
        InputDecision::PasteRejected => {
            *notice = Some("Paste rejected: the 64 KiB input limit was exceeded.".into());
            false
        }
    }
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
                        viewport: RatatuiViewport::Inline(INLINE_HEIGHT),
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

    #[test]
    fn attach_retry_uses_full_jitter_caps_and_exact_attempt_limit() {
        let now = Instant::now();
        let mut retry = AttachRetry::default();
        let expected_caps = [250, 500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000];
        for (attempt, cap_millis) in expected_caps.into_iter().enumerate() {
            let delay = retry.schedule(now, u64::MAX).unwrap();
            assert!(delay <= Duration::from_millis(cap_millis));
            assert_eq!(retry.attempts, u8::try_from(attempt + 1).unwrap());
        }
        assert_eq!(retry.schedule(now, 0), None);
        assert!(retry.deadline.is_none());
    }

    #[test]
    fn attach_retry_honors_elapsed_limit_and_reset() {
        let now = Instant::now();
        let mut retry = AttachRetry::default();
        assert_eq!(retry.schedule(now, 0), Some(Duration::ZERO));
        assert!(retry.due(now));
        retry.take_deadline();
        assert!(!retry.due(now));
        assert_eq!(
            retry.schedule(now + RETRY_MAX_ELAPSED, 0),
            None,
            "the ninety-second bound is inclusive"
        );
        retry.reset();
        assert_eq!(retry.attempts, 0);
        assert!(retry.started_at.is_none());
    }
}
