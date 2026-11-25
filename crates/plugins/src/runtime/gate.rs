use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use rquickjs::runtime::AsyncWeakRuntime;
use rquickjs::{AsyncContext, CaughtError, Ctx};

use super::{DEFAULT_EVENT_TIMEOUT, WorkerEnv};
use crate::api::{PluginApiState, PromptSenders};

pub(super) struct ExecGate {
    /// `0` = disarmed, `1` = immediate interrupt, `>= 2` = `(epoch_ms + 2)` deadline.
    deadline_ms: AtomicU64,
    deactivating: AtomicBool,
    interrupted: AtomicBool,
    global_shutdown: Arc<AtomicBool>,
    runtime_cancel: Arc<AtomicBool>,
    epoch: Instant,
}

impl ExecGate {
    pub(super) fn new(
        epoch: Instant,
        global_shutdown: Arc<AtomicBool>,
        runtime_cancel: Arc<AtomicBool>,
    ) -> Arc<Self> {
        Arc::new(Self {
            deadline_ms: AtomicU64::new(0),
            deactivating: AtomicBool::new(false),
            interrupted: AtomicBool::new(false),
            global_shutdown,
            runtime_cancel,
            epoch,
        })
    }

    fn now_tick(&self) -> u64 {
        (self.epoch.elapsed().as_millis() as u64).saturating_add(2)
    }

    pub(super) fn arm(&self, budget: Duration) {
        self.interrupted.store(false, Ordering::Release);
        self.deactivating.store(false, Ordering::Release);
        let target = self
            .now_tick()
            .saturating_add(budget.as_millis().max(1) as u64);
        self.deadline_ms.store(target.max(2), Ordering::Release);
    }

    pub(super) fn arm_immediate(&self) {
        self.interrupted.store(false, Ordering::Release);
        self.deactivating.store(false, Ordering::Release);
        self.deadline_ms.store(1, Ordering::Release);
    }

    /// Arms a bounded deadline specifically for `deactivate()` during shutdown or unload.
    /// Ignores `global_shutdown` and `runtime_cancel` so `deactivate()` can execute cleanly
    /// within the remaining shutdown budget.
    pub(super) fn arm_deactivate(&self, budget: Duration) {
        self.interrupted.store(false, Ordering::Release);
        let target = self
            .now_tick()
            .saturating_add(budget.as_millis().max(1) as u64);
        self.deadline_ms.store(target.max(2), Ordering::Release);
        self.deactivating.store(true, Ordering::Release);
    }

    pub(super) fn disarm(&self) {
        self.deactivating.store(false, Ordering::Release);
        self.deadline_ms.store(0, Ordering::Release);
    }

    pub(super) fn was_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Acquire)
    }

    pub(super) fn should_interrupt(&self) -> bool {
        if !self.deactivating.load(Ordering::Acquire)
            && (self.global_shutdown.load(Ordering::Acquire)
                || self.runtime_cancel.load(Ordering::Acquire))
        {
            self.interrupted.store(true, Ordering::Release);
            return true;
        }
        let dl = self.deadline_ms.load(Ordering::Acquire);
        if dl == 0 {
            return false;
        }
        if dl == 1 || self.now_tick() >= dl {
            self.interrupted.store(true, Ordering::Release);
            return true;
        }
        false
    }
}

/// Coordinates a persistent per-`LoadedRuntime` job driver task in `env.tasks`.
pub(super) struct RuntimeDriverState {
    unloaded: Cell<bool>,
    needs_pump: Cell<bool>,
    waker: RefCell<Option<std::task::Waker>>,
}

impl RuntimeDriverState {
    pub(super) fn new() -> Rc<Self> {
        Rc::new(Self {
            unloaded: Cell::new(false),
            needs_pump: Cell::new(false),
            waker: RefCell::new(None),
        })
    }

    pub(super) fn wake(&self) {
        self.needs_pump.set(true);
        if let Some(w) = self.waker.borrow_mut().take() {
            w.wake();
        }
    }

    /// Marks the owning `LoadedRuntime` as unloaded and wakes the driver so it drains/cancels
    /// any lingering `Promised` futures and exits once all strong `AsyncRuntime` handles drop.
    pub(super) fn stop(&self) {
        self.unloaded.set(true);
        self.wake();
    }
}

pub(super) struct ClearPersistentGuard {
    api: Rc<PluginApiState>,
    cancel_flag: Option<Arc<AtomicBool>>,
    prompt_cleanup: Option<(PromptSenders, String)>,
    armed: bool,
}

impl ClearPersistentGuard {
    pub(super) fn new(
        api: Rc<PluginApiState>,
        cancel_flag: Option<Arc<AtomicBool>>,
        prompt_cleanup: Option<(PromptSenders, String)>,
    ) -> Self {
        Self {
            api,
            cancel_flag,
            prompt_cleanup,
            armed: true,
        }
    }

    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ClearPersistentGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Some(flag) = &self.cancel_flag {
                flag.store(true, Ordering::Release);
            }
            if let Some((senders, owner_id)) = &self.prompt_cleanup {
                senders.borrow_mut().retain(|_, (owner, _)| owner != owner_id);
            }
            self.api.clear_persistent_handles();
        }
    }
}

struct DisarmGuard<'a>(&'a ExecGate);

impl Drop for DisarmGuard<'_> {
    fn drop(&mut self) {
        self.0.disarm();
    }
}

pub(super) struct SimpleGateFuture<Fut> {
    pub(super) fut: Fut,
    pub(super) gate: Arc<ExecGate>,
    pub(super) budget: Duration,
    pub(super) deactivate: bool,
}

impl<Fut: std::future::Future> std::future::Future for SimpleGateFuture<Fut> {
    type Output = Fut::Output;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        // SAFETY: projecting pinned field `fut`; `gate`, `budget`, and `deactivate` are not pinned.
        let (fut, gate, budget, deactivate) = unsafe {
            let this = self.get_unchecked_mut();
            (
                std::pin::Pin::new_unchecked(&mut this.fut),
                &this.gate,
                this.budget,
                this.deactivate,
            )
        };
        if deactivate {
            gate.arm_deactivate(budget);
        } else {
            gate.arm(budget);
        }
        let _disarm = DisarmGuard(gate);
        fut.poll(cx)
    }
}

struct WaitDriverWake {
    state: Rc<RuntimeDriverState>,
    yielded: bool,
}

impl std::future::Future for WaitDriverWake {
    type Output = ();

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<()> {
        if self.state.needs_pump.replace(false) || self.yielded {
            self.state.waker.borrow_mut().take();
            return Poll::Ready(());
        }
        *self.state.waker.borrow_mut() = Some(cx.waker().clone());
        self.yielded = true;
        Poll::Pending
    }
}

/// Persistent per-`LoadedRuntime` gated job driver.
///
/// - Never calls `block_on` (zero deadlock risk on `rt.inner.lock()`).
/// - Always arms `ExecGate` around `execute_pending_job()` and bounds microtask bursts.
/// - Parks with 0% CPU while idle or awaiting external futures (HTTP/OAuth/confirm/services).
/// - Owns a permanent `FuturesUnordered` slot waker so `rquickjs::Schedular`'s single
///   `AtomicWaker` is re-registered whenever any transient `GateFuture` finishes.
pub(super) async fn runtime_driver_task(
    env: Rc<WorkerEnv>,
    plugin_id: String,
    generation: u64,
    weak_rt: AsyncWeakRuntime,
    gate: Arc<ExecGate>,
    driver_state: Rc<RuntimeDriverState>,
) {
    const MAX_JOBS_PER_BURST: usize = 10_000;

    loop {
        if env.shutdown_flag.load(Ordering::Acquire) {
            break;
        }
        driver_state.needs_pump.set(false);
        // Stay alive until every strong `AsyncRuntime` handle (including any in-flight
        // `GateFuture` unwinding after disable/circuit-break) has dropped, so suspended
        // external futures in `rquickjs::Schedular` never lose their wakeup path.
        let Some(rt) = weak_rt.try_ref() else {
            break;
        };

        let burst_start = Instant::now();
        let mut jobs_ran = 0usize;
        let mut ran_any = false;
        let mut burst_err: Option<String> = None;

        loop {
            if env.shutdown_flag.load(Ordering::Acquire) {
                break;
            }
            let remaining = DEFAULT_EVENT_TIMEOUT.saturating_sub(burst_start.elapsed());
            if remaining.is_zero() || jobs_ran >= MAX_JOBS_PER_BURST {
                if !driver_state.unloaded.get() {
                    burst_err = Some(format!(
                        "plugin async job execution exceeded {}ms budget",
                        DEFAULT_EVENT_TIMEOUT.as_millis()
                    ));
                }
                break;
            }

            let job_res = SimpleGateFuture {
                fut: rt.execute_pending_job(),
                gate: Arc::clone(&gate),
                budget: remaining,
                deactivate: false,
            }
            .await;

            if gate.was_interrupted()
                && !driver_state.unloaded.get()
                && !env.shutdown_flag.load(Ordering::Acquire)
            {
                burst_err = Some(format!(
                    "plugin async job execution exceeded {}ms budget",
                    DEFAULT_EVENT_TIMEOUT.as_millis()
                ));
                break;
            }

            match job_res {
                Ok(true) => {
                    ran_any = true;
                    jobs_ran += 1;
                    continue;
                }
                Ok(false) => break,
                Err(err_ctx) => {
                    ran_any = true;
                    if !driver_state.unloaded.get() {
                        let msg: String = err_ctx
                            .0
                            .with(|ctx| format_caught_error(&ctx, rquickjs::Error::Exception))
                            .await;
                        burst_err = Some(msg);
                    }
                    break;
                }
            }
        }

        drop(rt);

        if env.shutdown_flag.load(Ordering::Acquire) {
            break;
        }
        if !driver_state.unloaded.get() {
            if let Some(msg) = burst_err {
                env.record_failure(&plugin_id, Some(generation), &msg);
            } else if ran_any {
                env.flush_plugin_side_effects(&plugin_id, Some(generation));
            }
        } else if weak_rt.try_ref().is_none() {
            break;
        }

        if env.shutdown_flag.load(Ordering::Acquire) {
            break;
        }

        WaitDriverWake {
            state: Rc::clone(&driver_state),
            yielded: false,
        }
        .await;
    }
}

pub(super) struct GateOptions {
    pub(super) budget: Duration,
    pub(super) driver_state: Option<Rc<RuntimeDriverState>>,
    pub(super) api: Option<Rc<PluginApiState>>,
    pub(super) write_depth: u32,
    pub(super) service_depth: usize,
}

impl GateOptions {
    pub(super) fn for_runtime(
        budget: Duration,
        driver_state: Rc<RuntimeDriverState>,
        api: Rc<PluginApiState>,
    ) -> Self {
        Self {
            budget,
            driver_state: Some(driver_state),
            api: Some(api),
            write_depth: 0,
            service_depth: 0,
        }
    }
}

pub(super) async fn run_with_gate<F, R>(
    ctx: &AsyncContext,
    gate: &Arc<ExecGate>,
    opts: GateOptions,
    f: F,
) -> Result<R>
where
    F: for<'js> AsyncFnOnce(Ctx<'js>) -> rquickjs::Result<R>,
    R: 'static,
{
    struct DepthRestoreGuard {
        api: Option<Rc<PluginApiState>>,
        prev_write: u32,
        prev_service: usize,
    }
    impl Drop for DepthRestoreGuard {
        fn drop(&mut self) {
            if let Some(api) = &self.api {
                api.write_depth.set(self.prev_write);
                api.active_service_depth.set(self.prev_service);
            }
        }
    }

    struct GateFuture<Fut> {
        fut: Fut,
        gate: Arc<ExecGate>,
        budget: Duration,
        cpu_used: Duration,
        driver_state: Option<Rc<RuntimeDriverState>>,
        api: Option<Rc<PluginApiState>>,
        write_depth: u32,
        service_depth: usize,
    }
    impl<Fut> Drop for GateFuture<Fut> {
        fn drop(&mut self) {
            // Wake the persistent per-runtime driver whenever any `GateFuture` completes or
            // is cancelled so it re-registers its permanent waker on `rquickjs::Schedular`
            // and drains any trailing microtasks.
            if let Some(driver) = &self.driver_state {
                driver.wake();
            }
        }
    }
    impl<Fut: std::future::Future> std::future::Future for GateFuture<Fut> {
        type Output = Fut::Output;
        fn poll(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> Poll<Self::Output> {
            // SAFETY: projecting pinned field `fut`; all other fields are `Unpin` / not pinned.
            let (fut, gate, budget, cpu_used, api, write_depth, service_depth) = unsafe {
                let this = self.get_unchecked_mut();
                (
                    std::pin::Pin::new_unchecked(&mut this.fut),
                    &this.gate,
                    this.budget,
                    &mut this.cpu_used,
                    this.api.as_ref().map(Rc::clone),
                    this.write_depth,
                    this.service_depth,
                )
            };
            let _depth_guard = if let Some(api_ref) = api {
                let prev_write = api_ref.write_depth.replace(write_depth);
                let prev_service = api_ref.active_service_depth.replace(service_depth);
                DepthRestoreGuard {
                    api: Some(api_ref),
                    prev_write,
                    prev_service,
                }
            } else {
                DepthRestoreGuard {
                    api: None,
                    prev_write: 0,
                    prev_service: 0,
                }
            };
            let remaining = budget.saturating_sub(*cpu_used);
            if remaining.is_zero() {
                gate.arm_immediate();
            } else {
                gate.arm(remaining);
            }
            let _disarm = DisarmGuard(gate);
            let poll_start = Instant::now();
            let res = fut.poll(cx);
            *cpu_used = cpu_used.saturating_add(poll_start.elapsed());
            res
        }
    }

    let inner = ctx.async_with(async move |ctx| match f(ctx.clone()).await {
        Ok(v) => Ok(v),
        Err(err) => Err(format_caught_error(&ctx, err)),
    });
    GateFuture {
        fut: inner,
        gate: Arc::clone(gate),
        budget: opts.budget,
        cpu_used: Duration::ZERO,
        driver_state: opts.driver_state,
        api: opts.api,
        write_depth: opts.write_depth,
        service_depth: opts.service_depth,
    }
    .await
    .map_err(|msg| anyhow!("{msg}"))
}

pub(super) fn format_caught_error<'js>(ctx: &Ctx<'js>, err: rquickjs::Error) -> String {
    match CaughtError::from_error(ctx, err) {
        CaughtError::Error(e) => format!("{e}"),
        CaughtError::Exception(ex) => {
            let msg = ex.message().unwrap_or_else(|| "JS Exception".to_owned());
            match ex.stack().filter(|s| !s.trim().is_empty()) {
                Some(stack) => format!("{msg}\n{stack}"),
                None => msg,
            }
        }
        CaughtError::Value(val) => {
            if let Some(s) = val.as_string().and_then(|s| s.to_string().ok()) {
                return s;
            }
            if val.type_of() == rquickjs::Type::Unknown {
                return "Internal plugin host error while executing JS callback".to_owned();
            }
            if let Some(obj) = val.as_object()
                && let Ok(msg) = obj.get::<_, String>("message")
                && !msg.trim().is_empty()
            {
                return msg;
            }
            if matches!(val.type_of(), rquickjs::Type::Object | rquickjs::Type::Array)
                && let Ok(Some(json_str)) = ctx.json_stringify(val.clone())
                && let Ok(s) = json_str.to_string()
            {
                return s;
            }
            format!("{val:?}")
        }
    }
}

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
