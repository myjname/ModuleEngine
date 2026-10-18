//! Owned, interruptible actor deadlines. Windows uses a high-resolution timer
//! without changing process/system timer resolution; standard parking keeps an
//! owned predicate independent of the thread token used by other libraries.
#[cfg(windows)]
use std::sync::atomic::AtomicU64;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub(crate) struct WakeSignal {
    pending: AtomicBool,
    #[cfg(windows)]
    event: Option<windows::Handle>,
    #[cfg(windows)]
    event_error: AtomicU64,
}
impl WakeSignal {
    pub(crate) fn new() -> Self {
        #[cfg(windows)]
        {
            let (event, error) = match windows::Handle::event() {
                Ok(event) => (Some(event), 0),
                Err(error) => (None, u64::from(error) + 1),
            };
            Self {
                pending: AtomicBool::new(false),
                event,
                event_error: AtomicU64::new(error),
            }
        }
        #[cfg(not(windows))]
        Self {
            pending: AtomicBool::new(false),
        }
    }

    pub(crate) fn notify(&self, owner: &thread::Thread) {
        // The Thread token is shared with blocking channels/libraries. Retain
        // an owner-specific permit even if those consume the standard token.
        self.pending.store(true, Ordering::Release);
        owner.unpark();
        #[cfg(windows)]
        if self.event_error.load(Ordering::Acquire) == 0
            && let Some(event) = &self.event
            && let Err(error) = event.set()
        {
            self.event_error
                .store(u64::from(error) + 1, Ordering::Release);
        }
    }

    #[cfg(windows)]
    fn error(&self) -> Option<u32> {
        self.event_error
            .load(Ordering::Acquire)
            .checked_sub(1)
            .map(|error| error as u32)
    }

    #[cfg(all(windows, test))]
    pub(crate) fn inject_notification_failure(&self, error: u32) {
        // Exercise the failure transition without closing a handle that an
        // owner may currently be waiting on.
        self.event_error
            .store(u64::from(error) + 1, Ordering::Release);
    }
}

pub(crate) struct DeadlineWait {
    signal: Arc<WakeSignal>,
    #[cfg(windows)]
    timer: Option<windows::Timer>,
    #[cfg(windows)]
    failure: Option<String>,
}
impl DeadlineWait {
    pub(crate) fn new(signal: Arc<WakeSignal>) -> Self {
        #[cfg(windows)]
        {
            let result = match signal.error() {
                Some(error) => {
                    let operation = if signal.event.is_some() {
                        "SetEvent"
                    } else {
                        "CreateEventW"
                    };
                    Err(format!("{operation}: Windows error {error}"))
                }
                None => windows::Timer::new(),
            };
            let (timer, failure) = match result {
                Ok(timer) => (Some(timer), None),
                Err(error) => (None, Some(error)),
            };
            Self {
                signal,
                timer,
                failure,
            }
        }
        #[cfg(not(windows))]
        {
            Self { signal }
        }
    }

    pub(crate) fn strategy(&self) -> &'static str {
        #[cfg(windows)]
        {
            if self.timer.is_some() && self.signal.error().is_none() {
                "windows-high-resolution-waitable-timer"
            } else {
                "std-park-fallback"
            }
        }
        #[cfg(not(windows))]
        {
            "std-park"
        }
    }

    pub(crate) fn error(&self) -> Option<String> {
        #[cfg(windows)]
        {
            self.failure.clone().or_else(|| {
                self.signal
                    .error()
                    .map(|error| format!("SetEvent: Windows error {error}"))
            })
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    pub(crate) fn wait(&mut self, duration: Duration) {
        if duration.is_zero() {
            thread::yield_now();
            return;
        }
        #[cfg(windows)]
        if let Some(timer) = &self.timer {
            let started = Instant::now();
            let result = match self.signal.error() {
                Some(error) => Err(format!("SetEvent: Windows error {error}")),
                None => match &self.signal.event {
                    Some(event) => timer.wait(event, duration),
                    None => Err("Owned Windows worker wake event is unavailable".into()),
                },
            };
            match result {
                Ok(()) => {
                    self.signal.pending.swap(false, Ordering::AcqRel);
                    return;
                }
                Err(error) => {
                    self.failure = Some(error);
                    self.timer = None;
                    // The owned predicate survives other libraries consuming
                    // the thread token. Never wait native elapsed time twice.
                    self.park_standard(duration.saturating_sub(started.elapsed()));
                    return;
                }
            }
        }
        self.park_standard(duration);
    }

    fn park_standard(&self, duration: Duration) {
        let started = Instant::now();
        loop {
            if self.signal.pending.swap(false, Ordering::AcqRel) {
                return;
            }
            let remaining = duration.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return;
            }
            // No unknown/library code between checking the owned permit and
            // park. A concurrent notify retains the standard token; stale or
            // spurious returns recheck the predicate against the same start.
            thread::park_timeout(remaining);
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::{
        ffi::c_void,
        ptr::NonNull,
        time::{Duration, Instant},
    };
    type RawHandle = *mut c_void;
    type TimerApc = Option<unsafe extern "system" fn(*const c_void, u32, u32)>;
    // Policy for detecting an unresponsive timer, not timer precision. Coarse
    // wait timeouts can expire early; only monotonic elapsed retires the timer.
    const WATCHDOG_GRACE: Duration = Duration::from_millis(100);
    const MIN_BLOCKING_TIMEOUT_MS: u32 = 100;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateEventW(
            attributes: *const c_void,
            manual: i32,
            initial: i32,
            name: *const u16,
        ) -> RawHandle;
        fn SetEvent(event: RawHandle) -> i32;
        fn CreateWaitableTimerExW(
            attributes: *const c_void,
            name: *const u16,
            flags: u32,
            access: u32,
        ) -> RawHandle;
        fn SetWaitableTimerEx(
            timer: RawHandle,
            due: *const i64,
            period: i32,
            callback: TimerApc,
            argument: *const c_void,
            context: *const c_void,
            tolerable_delay: u32,
        ) -> i32;
        fn CancelWaitableTimer(timer: RawHandle) -> i32;
        fn WaitForMultipleObjects(
            count: u32,
            handles: *const RawHandle,
            all: i32,
            timeout: u32,
        ) -> u32;
        fn CloseHandle(handle: RawHandle) -> i32;
        fn GetLastError() -> u32;
    }

    pub(super) struct Handle {
        raw: NonNull<c_void>,
        #[cfg(test)]
        close_outcome: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    }
    // Kernel event/timer handles can be signaled/waited on across threads. The
    // unique owner closes only after the Arc-held wake users/owner wait release it.
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}
    impl Handle {
        fn take(raw: RawHandle) -> Result<Self, u32> {
            NonNull::new(raw)
                .map(|raw| Self {
                    raw,
                    #[cfg(test)]
                    close_outcome: None,
                })
                .ok_or_else(|| unsafe { GetLastError() })
        }
        pub(super) fn event() -> Result<Self, u32> {
            // Auto-reset event retains one pre-wait signal; never ResetEvent.
            Self::take(unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) })
        }
        pub(super) fn set(&self) -> Result<(), u32> {
            if unsafe { SetEvent(self.raw.as_ptr()) } != 0 {
                Ok(())
            } else {
                Err(unsafe { GetLastError() })
            }
        }
        #[cfg(test)]
        pub(super) fn watch_close(&mut self) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
            let outcome = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
            self.close_outcome = Some(outcome.clone());
            outcome
        }
    }
    impl Drop for Handle {
        fn drop(&mut self) {
            let result = unsafe { CloseHandle(self.raw.as_ptr()) };
            #[cfg(test)]
            if let Some(outcome) = &self.close_outcome {
                use std::sync::atomic::Ordering;
                let result = if result != 0 {
                    1
                } else {
                    u64::from(unsafe { GetLastError() }) + 2
                };
                outcome.store(result, Ordering::Release);
            }
            #[cfg(not(test))]
            let _ = result;
        }
    }

    pub(super) struct Timer(Handle);
    impl Timer {
        pub(super) fn new() -> Result<Self, String> {
            // CREATE_WAITABLE_TIMER_HIGH_RESOLUTION; auto-reset, minimal
            // TIMER_MODIFY_STATE | SYNCHRONIZE access, no named/shared timer.
            Handle::take(unsafe {
                CreateWaitableTimerExW(std::ptr::null(), std::ptr::null(), 2, 0x0010_0002)
            })
            .map(Self)
            .map_err(|error| {
                format!("CreateWaitableTimerExW(high-resolution): Windows error {error}")
            })
        }
        pub(super) fn wait(&self, event: &Handle, duration: Duration) -> Result<(), String> {
            let due = relative_due(duration);
            let watchdog = duration.saturating_add(WATCHDOG_GRACE);
            let mut timeout_ms = safety_timeout(watchdog);
            let started = Instant::now();
            if unsafe {
                SetWaitableTimerEx(
                    self.0.raw.as_ptr(),
                    &due,
                    0,
                    None,
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                )
            } == 0
            {
                return Err(format!("SetWaitableTimerEx: Windows error {}", unsafe {
                    GetLastError()
                }));
            }
            let handles = [event.raw.as_ptr(), self.0.raw.as_ptr()];
            // Keep the same arm and start across coarse early timeouts: event
            // and timer signals remain live. Every retry is a positive blocking
            // wait, with a 100ms floor rather than a shrinking sub-tick poll.
            // Logical watchdog=duration+100ms. A final retry may extend it by
            // up to the 100ms floor plus OS clock/scheduling delay. A healthy
            // timer/event normally releases the wait before that watchdog.
            let (result, error, elapsed) = loop {
                let result = wait_objects(&handles, timeout_ms);
                let error = if result == u32::MAX {
                    unsafe { GetLastError() }
                } else {
                    0
                };
                let elapsed = started.elapsed();
                if result != 258 || elapsed >= watchdog {
                    break (result, error, elapsed);
                }
                timeout_ms = safety_timeout(watchdog.saturating_sub(elapsed));
            };
            let cancelled = unsafe { CancelWaitableTimer(self.0.raw.as_ptr()) };
            let cancel_error = if cancelled == 0 {
                unsafe { GetLastError() }
            } else {
                0
            };
            match result {
                0 | 1 if cancelled != 0 => Ok(()),
                0 | 1 => Err(format!("CancelWaitableTimer: Windows error {cancel_error}")),
                258 => Err(format!(
                    "High-resolution worker wait reached its safety timeout: requested_ns={}, elapsed_ns={}, watchdog_ns={}, timeout_ms={timeout_ms}, wait_result={result}",
                    duration.as_nanos(),
                    elapsed.as_nanos(),
                    watchdog.as_nanos()
                )),
                u32::MAX => Err(format!("WaitForMultipleObjects: Windows error {error}")),
                other => Err(format!(
                    "WaitForMultipleObjects returned unexpected value {other}"
                )),
            }
        }
        #[cfg(test)]
        pub(super) fn watch_close(&mut self) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
            self.0.watch_close()
        }
    }
    impl Drop for Timer {
        fn drop(&mut self) {
            unsafe { CancelWaitableTimer(self.0.raw.as_ptr()) };
        }
    }
    #[cfg(test)]
    std::thread_local! {
        static EARLY_TIMEOUT_ONCE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    #[cfg(test)]
    pub(super) fn inject_early_timeout_once() {
        EARLY_TIMEOUT_ONCE.with(|pending| pending.set(true));
    }
    fn wait_objects(handles: &[RawHandle; 2], timeout_ms: u32) -> u32 {
        // Test only: model the documented coarse timeout arriving before its
        // requested interval. The real timer remains armed and unconsumed.
        #[cfg(test)]
        if EARLY_TIMEOUT_ONCE.with(|pending| pending.replace(false)) {
            return 258;
        }
        unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, timeout_ms) }
    }
    pub(super) fn relative_due(duration: Duration) -> i64 {
        -(duration.as_nanos().div_ceil(100).clamp(1, i64::MAX as u128) as i64)
    }
    fn safety_timeout(duration: Duration) -> u32 {
        duration.as_nanos().div_ceil(1_000_000).clamp(
            u128::from(MIN_BLOCKING_TIMEOUT_MS),
            u128::from(u32::MAX - 1),
        ) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Instant};

    #[cfg(windows)]
    #[test]
    fn early_coarse_timeout_retries_the_same_armed_timer_without_falling_back() {
        // A fresh thread excludes stale standard permits and isolates the
        // one-shot test hook from other tests. It never signals the real event.
        thread::spawn(|| {
            let signal = Arc::new(WakeSignal::new());
            let mut wait = DeadlineWait::new(signal);
            if wait.strategy() != "windows-high-resolution-waitable-timer" {
                assert!(wait.error().is_some());
                return;
            }
            windows::inject_early_timeout_once();
            let started = Instant::now();
            wait.wait(Duration::from_millis(20));
            let elapsed = started.elapsed();
            assert_eq!(wait.strategy(), "windows-high-resolution-waitable-timer");
            assert!(wait.error().is_none());
            assert!(elapsed >= Duration::from_millis(10));
            assert!(elapsed < Duration::from_secs(5));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn notification_before_wait_is_retained_and_coalesced_without_a_lost_wake() {
        let signal = Arc::new(WakeSignal::new());
        let mut wait = DeadlineWait::new(signal.clone());
        signal.notify(&thread::current());
        signal.notify(&thread::current());
        let started = Instant::now();
        wait.wait(Duration::from_secs(10));
        assert!(started.elapsed() < Duration::from_secs(5));
        #[cfg(windows)]
        if wait.strategy() == "windows-high-resolution-waitable-timer" {
            let started = Instant::now();
            wait.wait(Duration::from_millis(20));
            assert!(started.elapsed() >= Duration::from_millis(10));
            assert!(started.elapsed() < Duration::from_secs(5));
        }
    }

    #[test]
    fn owned_notification_survives_an_unrelated_standard_park() {
        let signal = Arc::new(WakeSignal::new());
        #[cfg(windows)]
        signal.inject_notification_failure(6);
        let owned = signal.clone();
        let (done_tx, done_rx) = mpsc::channel();
        // Fresh owner: no other library/parent supplies a second permit before
        // the bounded parent observation. Rescue is sent only afterward.
        let owner = thread::spawn(move || {
            let mut wait = DeadlineWait::new(owned.clone());
            owned.notify(&thread::current());
            // Model a blocking channel/library call consuming the shared
            // Thread token before the owner's own deadline wait begins.
            thread::park();
            wait.wait(Duration::from_secs(10));
            let _ = done_tx.send(());
        });
        let result = done_rx.recv_timeout(Duration::from_secs(5));
        signal.notify(owner.thread());
        drop(done_rx);
        owner.join().unwrap();
        assert!(
            result.is_ok(),
            "A library consumed the std permit, but the owned wake must survive: {result:?}"
        );
    }

    #[test]
    fn notification_racing_the_wait_releases_the_owner_and_joins() {
        let signal = Arc::new(WakeSignal::new());
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let (done_tx, done_rx) = mpsc::sync_channel(0);
        let owned = signal.clone();
        let owner = thread::spawn(move || {
            let mut wait = DeadlineWait::new(owned);
            for _ in 0..8 {
                if ready_tx.send(()).is_err() {
                    break;
                }
                wait.wait(Duration::from_secs(10));
                if done_tx.send(()).is_err() {
                    break;
                }
            }
        });
        let mut failure = None;
        for _ in 0..8 {
            if let Err(error) = ready_rx.recv_timeout(Duration::from_secs(5)) {
                failure = Some(error);
                break;
            }
            signal.notify(owner.thread());
            if let Err(error) = done_rx.recv_timeout(Duration::from_secs(5)) {
                failure = Some(error);
                break;
            }
        }
        signal.notify(owner.thread());
        drop(ready_rx);
        drop(done_rx);
        owner.join().unwrap();
        assert!(
            failure.is_none(),
            "Owner did not observe its wake: {failure:?}"
        );
    }

    #[test]
    fn timeout_is_bounded_and_zero_wait_does_not_spin_internally() {
        // A fresh owner avoids a standard park permit from another test.
        thread::spawn(|| {
            let mut wait = DeadlineWait::new(Arc::new(WakeSignal::new()));
            let started = Instant::now();
            wait.wait(Duration::from_millis(20));
            #[cfg(windows)]
            if wait.strategy() == "windows-high-resolution-waitable-timer" {
                assert!(started.elapsed() >= Duration::from_millis(10));
            }
            assert!(started.elapsed() < Duration::from_secs(5));
            wait.wait(Duration::ZERO);
            #[cfg(not(windows))]
            assert_eq!(wait.strategy(), "std-park");
            #[cfg(windows)]
            assert!(matches!(
                wait.strategy(),
                "windows-high-resolution-waitable-timer" | "std-park-fallback"
            ));
        })
        .join()
        .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_relative_deadlines_round_up_and_drop_releases_owned_handles() {
        assert_eq!(windows::relative_due(Duration::from_nanos(1)), -1);
        assert_eq!(windows::relative_due(Duration::from_nanos(100)), -1);
        assert_eq!(windows::relative_due(Duration::from_nanos(101)), -2);
        assert_eq!(windows::relative_due(Duration::MAX), -i64::MAX);
        let mut signal = Arc::new(WakeSignal::new());
        let event_close = Arc::get_mut(&mut signal)
            .unwrap()
            .event
            .as_mut()
            .unwrap()
            .watch_close();
        let mut wait = DeadlineWait::new(signal.clone());
        let timer_close = wait.timer.as_mut().map(windows::Timer::watch_close);
        drop(wait);
        if let Some(closed) = timer_close {
            assert_eq!(closed.load(Ordering::Acquire), 1);
        }
        drop(signal);
        assert_eq!(event_close.load(Ordering::Acquire), 1);
    }

    #[cfg(windows)]
    #[test]
    fn native_signal_failure_reports_fallback_and_keeps_the_standard_wake_permit() {
        thread::spawn(|| {
            let signal = Arc::new(WakeSignal::new());
            let mut wait = DeadlineWait::new(signal.clone());
            // Fault injection marks this owned signal unavailable, without
            // closing a handle another thread could still be waiting on.
            signal.event_error.store(7, Ordering::Release);
            signal.notify(&thread::current());
            let started = Instant::now();
            wait.wait(Duration::from_secs(10));
            assert!(started.elapsed() < Duration::from_secs(5));
            assert_eq!(wait.strategy(), "std-park-fallback");
            assert!(wait.error().unwrap().contains("Windows error"));
        })
        .join()
        .unwrap();
    }
}
