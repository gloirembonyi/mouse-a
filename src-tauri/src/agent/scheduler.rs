// Kudu Desktop agent - sharing ONE mouse, keyboard and screen between several runs.
//
// Runs overlap: the user's own chat task ("main:<chat>"), the Pilot ("copilot") and tasks
// the browser agent hands over ("browser:<id>"). Commands, files, web and model calls run
// side by side freely - only GUI actions need the input. Without arbitration two agents
// interleaved clicks and keystrokes (A clicks into Notepad, B clicks a browser, A's text
// lands in the browser), and each reset the other's notion of the target window.
//
// The input lease:
//   • a GUI action (click, type, key, scroll, focus, open) takes the lease first;
//   • the lease is STICKY: its holder keeps it for STICKY after each action, so a
//     click-then-type sequence of one run is never split by another run's click;
//   • the user's own runs go first - a browser-requested run waits while a user run
//     is waiting for the lease, so a hand-off never interrupts what the user started;
//   • the lease is dropped when its run ends (end_run), and a waiter gives up after
//     MAX_WAIT with a clear message instead of hanging the run.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const STICKY: Duration = Duration::from_secs(4);
const MAX_WAIT: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(120);

struct Lease {
    owner: Option<String>,
    last_used: Option<Instant>,
}

impl Lease {
    fn idle_for(&self) -> Duration {
        self.last_used.map(|t| t.elapsed()).unwrap_or(Duration::MAX)
    }
}

static LEASE: Mutex<Lease> = Mutex::new(Lease { owner: None, last_used: None });
/// User runs currently waiting for the lease - hand-offs yield to them.
static USER_WAITING: AtomicUsize = AtomicUsize::new(0);

fn is_user_run(source: &str) -> bool {
    !source.starts_with("browser:")
}

/// Who holds the input right now (for the Esc key and the Agents view).
pub fn input_owner() -> Option<String> {
    let lease = LEASE.lock().unwrap();
    match &lease.owner {
        Some(o) if lease.idle_for() < STICKY => Some(o.clone()),
        _ => None,
    }
}

/// Try once; true when `source` now holds the lease.
fn try_take(source: &str) -> bool {
    if !is_user_run(source) && USER_WAITING.load(Ordering::SeqCst) > 0 {
        return false; // the user's own task is queued for the screen - it goes first
    }
    let mut lease = LEASE.lock().unwrap();
    let free = match &lease.owner {
        None => true,
        Some(o) if o == source => true,
        Some(owner) => lease.idle_for() >= STICKY || !crate::agent::is_run_active(owner),
    };
    if free {
        lease.owner = Some(source.to_string());
        lease.last_used = Some(Instant::now());
    }
    free
}

/// Wait for the mouse/keyboard. Ok(waited) once held; Err when it never freed up.
/// `on_wait` is called once if the run has to wait, with who it is waiting for.
pub async fn acquire_input(source: &str, mut on_wait: impl FnMut(&str)) -> Result<bool, String> {
    if try_take(source) {
        return Ok(false);
    }
    let user = is_user_run(source);
    if user {
        USER_WAITING.fetch_add(1, Ordering::SeqCst);
    }
    let started = Instant::now();
    let mut announced = false;
    let result = loop {
        if try_take(source) {
            break Ok(true);
        }
        if crate::agent::is_cancel_pending(source) {
            break Err("Stopped.".to_string());
        }
        if started.elapsed() >= MAX_WAIT {
            break Err(
                "Another task kept the mouse and keyboard busy, so this screen action was not performed. \
Do the parts that need no mouse or keyboard (files, commands, web), then try this step again."
                    .to_string(),
            );
        }
        if !announced {
            announced = true;
            let holder = LEASE.lock().unwrap().owner.clone().unwrap_or_default();
            on_wait(&holder);
        }
        tokio::time::sleep(POLL).await;
    };
    if user {
        USER_WAITING.fetch_sub(1, Ordering::SeqCst);
    }
    result
}

/// Mark the end of a GUI action: the sticky window restarts from now.
pub fn touch_input(source: &str) {
    let mut lease = LEASE.lock().unwrap();
    if lease.owner.as_deref() == Some(source) {
        lease.last_used = Some(Instant::now());
    }
}

/// The run is over: give the input back at once.
pub fn release_input(source: &str) {
    let mut lease = LEASE.lock().unwrap();
    if lease.owner.as_deref() == Some(source) {
        lease.owner = None;
    }
}

/// Human-readable name of a run for "waiting for …" messages.
pub fn describe_source(source: &str) -> &'static str {
    if source.starts_with("browser:") {
        "a task from the browser agent"
    } else if source.starts_with("copilot") {
        "the Pilot"
    } else {
        "your other task"
    }
}
