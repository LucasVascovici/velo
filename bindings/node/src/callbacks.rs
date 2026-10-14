//! Change events and merge drivers, with the rule that keeps both safe.
//!
//! # Threading
//!
//! Every core call runs on the libuv pool, so a merge driver runs there, under
//! the repository lock, possibly on a rayon worker. The driver is a
//! `ThreadsafeFunction`: the worker asks the JS thread to run the function and
//! blocks until the answer comes back over a channel. The JS thread is free to
//! answer because the method that started the merge is asynchronous.
//!
//! # Re-entrancy
//!
//! The JS function runs while the repository is busy. A call it makes on the
//! same handle is refused, not queued: while the function runs, a flag is set
//! (on the JS thread, around the call), and every method called then is
//! rejected with `code: 'Reentrant'`. A flag read at call time, on the JS
//! thread, is what keeps an unrelated call made while a merge is pending from
//! being refused.
//!
//! Events cannot re-enter by construction: the core listener only queues them,
//! and the queue is delivered when the task resolves, on the JS thread, after
//! the core call has finished.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::Env;
use velo_core::events::{Event, Listener, Ref};
use velo_core::merge::{MergeDriver, MergeResult};
use velo_core::{Drivers, Error as CoreError};

use napi::{Env as NapiEnv, Task};
use napi_derive::napi;

use crate::errors::to_js;
use crate::repo::{CoreResult, Repo, Ticket};

/// The message of every refused re-entrant call.
pub const REENTRANT: &str =
    "a merge driver or event callback must not call back into the repository while it is busy";

/// Marks an `InvalidInput` detail that really is a failed merge driver, so the
/// one conversion in `errors.rs` can give it its own code.
pub const DRIVER_FAILED: &str = "merge driver failed: ";

/// The error a refused re-entrant call rejects with (`code: 'Reentrant'`).
pub fn reentrant() -> CoreError {
    CoreError::invalid(REENTRANT)
}

fn driver_failed(message: &str) -> CoreError {
    CoreError::invalid(format!("{DRIVER_FAILED}{message}"))
}

/// Events committed through the handle, waiting for the core call to return.
#[derive(Default)]
pub struct Queue(Mutex<Vec<Event>>);

/// The listener installed on the core handle; it only queues.
pub struct QueueListener(Arc<Queue>);

impl Listener for QueueListener {
    fn notify(&self, event: &Event) {
        self.0
             .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event.clone());
    }
}

type Args = FnArgs<(String, String, String)>;
type DriverFn = ThreadsafeFunction<Args, Vec<String>, Args, napi::Status, false, true>;
type EventFn = FunctionRef<Object<'static>, Unknown<'static>>;

/// Turn the driver's return into `["ok", text]`, `["conflict"]` or
/// `["error", message]`, so a throw or a wrong type is data the worker can
/// read rather than an uncaught exception.
const DRIVER_WRAPPER: &str = r#"(f) => (a, o, t) => {
  try {
    const v = f(a, o, t);
    if (typeof v === 'string') return ['ok', v];
    if (v === null || v === undefined) return ['conflict', ''];
    return ['error', 'a merge driver must return a string, or null for a conflict'];
  } catch (e) {
    return ['error', String(e && e.message !== undefined ? e.message : e)];
  }
}"#;

/// An event callback that throws is reported as an uncaught exception, later,
/// and does not fail the write it announces.
const EVENT_WRAPPER: &str = r#"(f) => (e) => {
  try { f(e); } catch (err) { process.nextTick(() => { throw err; }); }
}"#;

/// A merge driver written in JavaScript.
pub struct JsDriver {
    tsfn: Mutex<DriverFn>,
    flag: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
}

impl JsDriver {
    fn fail(&self, message: String) {
        let mut slot = self.error.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some(message);
        }
    }
}

/// `MergeDriver` for a shared `JsDriver`.
struct JsDriverRef(Arc<JsDriver>);

impl MergeDriver for JsDriverRef {
    fn name(&self) -> &str {
        "javascript"
    }

    fn merge(&self, ancestor: &str, ours: &str, theirs: &str) -> MergeResult {
        let conflict = || MergeResult::Conflicted(Vec::new());
        let driver = &self.0;
        // After the first failure every later path is a conflict too, so the
        // merge cannot go on to record anything built on a failed driver.
        if driver
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            return conflict();
        }
        let (tx, rx) = mpsc::channel();
        let flag = driver.flag.clone();
        let status = driver
            .tsfn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .call_with_return_value(
                (ancestor.to_string(), ours.to_string(), theirs.to_string()).into(),
                ThreadsafeFunctionCallMode::Blocking,
                move |ret, _env| {
                    flag.store(false, Ordering::SeqCst);
                    let _ = tx.send(ret.map_err(|e| e.reason.clone()));
                    Ok(())
                },
            );
        if status != napi::Status::Ok {
            driver.fail(format!("could not call the merge driver ({status})"));
            return conflict();
        }
        match rx.recv() {
            Ok(Ok(parts)) => match parts.first().map(String::as_str) {
                Some("ok") => MergeResult::Clean(parts.get(1).cloned().unwrap_or_default()),
                Some("conflict") => conflict(),
                _ => {
                    driver.fail(parts.get(1).cloned().unwrap_or_default());
                    conflict()
                }
            },
            Ok(Err(reason)) => {
                driver.fail(reason);
                conflict()
            }
            Err(_) => {
                driver.fail("the merge driver was never called".to_string());
                conflict()
            }
        }
    }
}

/// What `setMergeDriver` was given.
#[derive(Clone)]
pub enum Rule {
    Builtin(&'static str),
    Custom(Arc<JsDriver>),
}

/// Everything a handle keeps besides the core repository.
pub struct Hooks {
    /// Where the repository was opened, to reopen it with other merge drivers.
    root: std::path::PathBuf,
    queue: Arc<Queue>,
    callbacks: Mutex<Vec<EventFn>>,
    rules: Mutex<Vec<(String, Rule)>>,
    /// Set while a JS driver function is running on the JS thread.
    pub in_callback: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
}

impl Hooks {
    pub fn new(root: &std::path::Path) -> Hooks {
        Hooks {
            root: std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf()),
            queue: Arc::new(Queue::default()),
            callbacks: Mutex::new(Vec::new()),
            rules: Mutex::new(Vec::new()),
            in_callback: Arc::new(AtomicBool::new(false)),
            error: Arc::new(Mutex::new(None)),
        }
    }

    pub fn listener(&self) -> QueueListener {
        QueueListener(self.queue.clone())
    }

    /// Clear the driver error slot, before a core call.
    pub fn reset_error(&self) {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    /// The failure a driver recorded during the call, as a rejection.
    pub fn driver_error(&self) -> Option<CoreError> {
        self.error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .map(|m| driver_failed(&m))
    }

    pub fn has_custom_driver(&self) -> bool {
        self.rules
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|(_, r)| matches!(r, Rule::Custom(_)))
    }

    pub fn add_callback(&self, callback: EventFn) {
        self.callbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(callback);
    }

    /// Wrap a JS merge function for use from worker threads. JS thread only.
    pub fn custom_rule(
        &self,
        env: &Env,
        function: Function<'_, Args, Unknown<'_>>,
    ) -> Result<Rule> {
        let make: Function<Function<Args, Unknown>, Function<Args, Vec<String>>> =
            env.run_script(DRIVER_WRAPPER)?;
        let wrapped = make.call(function)?;
        let flag = self.in_callback.clone();
        let tsfn = wrapped
            .build_threadsafe_function::<Args>()
            .callee_handled::<false>()
            .weak::<true>()
            .build_callback(move |ctx| {
                flag.store(true, Ordering::SeqCst);
                Ok(ctx.value)
            })?;
        Ok(Rule::Custom(Arc::new(JsDriver {
            tsfn: Mutex::new(tsfn),
            flag: self.in_callback.clone(),
            error: self.error.clone(),
        })))
    }

    /// The rules with `add` appended, as core drivers.
    pub fn drivers_with(
        &self,
        add: Option<(String, Rule)>,
    ) -> CoreResult<(Drivers, Vec<(String, Rule)>)> {
        let mut list = self.rules.lock().unwrap_or_else(|e| e.into_inner()).clone();
        list.extend(add);
        let mut drivers = Drivers::new();
        for (pattern, rule) in &list {
            drivers = match rule {
                Rule::Builtin("json") => drivers.with(pattern, velo_core::merge::JsonDriver)?,
                Rule::Builtin("yaml") => drivers.with(pattern, velo_core::merge::YamlDriver)?,
                Rule::Builtin(_) => drivers.with(pattern, velo_core::merge::TomlDriver)?,
                Rule::Custom(d) => drivers.with(pattern, JsDriverRef(d.clone()))?,
            };
        }
        Ok((drivers, list))
    }

    pub fn set_rules(&self, list: Vec<(String, Rule)>) {
        *self.rules.lock().unwrap_or_else(|e| e.into_inner()) = list;
    }

    /// Deliver queued events to the callbacks. JS thread, after the core call.
    pub fn flush(&self, env: &Env) {
        let events = std::mem::take(&mut *self.queue.0.lock().unwrap_or_else(|e| e.into_inner()));
        if events.is_empty() {
            return;
        }
        let callbacks = self.callbacks.lock().unwrap_or_else(|e| e.into_inner());
        for event in &events {
            for callback in callbacks.iter() {
                let Ok(function) = callback.borrow_back(env) else {
                    continue;
                };
                let Ok(object) = event_object(env, event) else {
                    continue;
                };
                // The wrapper catches what the callback throws.
                let _ = function.call(object);
            }
        }
    }
}

/// Wrap and keep a JS event callback. JS thread only.
pub fn event_callback(
    env: &Env,
    function: Function<'_, Object<'_>, Unknown<'_>>,
) -> Result<EventFn> {
    let make: Function<Function<Object, Unknown>, Function<Object, Unknown>> =
        env.run_script(EVENT_WRAPPER)?;
    make.call(function)?.create_ref()
}

/// Parse the driver argument of `setMergeDriver`.
pub fn builtin(name: &str) -> CoreResult<Rule> {
    match name {
        "json" => Ok(Rule::Builtin("json")),
        "yaml" => Ok(Rule::Builtin("yaml")),
        "toml" => Ok(Rule::Builtin("toml")),
        other => Err(CoreError::invalid(format!(
            "'{}' is not a merge driver; expected 'json', 'yaml', 'toml' or a function.",
            other
        ))),
    }
}

fn event_object<'env>(env: &'env Env, event: &Event) -> Result<Object<'env>> {
    let mut o = Object::new(env)?;
    let id = |s: &velo_core::SnapshotId| s.as_str().to_string();
    match event {
        Event::Saved {
            snapshot,
            branch,
            parent,
            merge_parent,
        } => {
            o.set("kind", "Saved")?;
            o.set("snapshot", id(snapshot))?;
            o.set("branch", branch.as_str())?;
            o.set("parent", parent.as_ref().map(id))?;
            o.set("mergeParent", merge_parent.as_ref().map(id))?;
        }
        Event::Merged {
            snapshot,
            into,
            ours,
            theirs,
        } => {
            o.set("kind", "Merged")?;
            o.set("snapshot", id(snapshot))?;
            o.set("branch", into.as_str())?;
            o.set("ours", id(ours))?;
            o.set("theirs", id(theirs))?;
        }
        Event::RefMoved {
            reference,
            from,
            to,
        } => {
            o.set("kind", "RefMoved")?;
            match reference {
                Ref::Branch(b) => {
                    o.set("refKind", "branch")?;
                    o.set("name", b.as_str())?;
                }
                Ref::Tag(t) => {
                    o.set("refKind", "tag")?;
                    o.set("name", t.as_str())?;
                }
                _ => o.set("refKind", "unknown")?,
            }
            o.set("from", from.as_ref().map(id))?;
            o.set("to", to.as_ref().map(id))?;
        }
        Event::Imported { snapshots } => {
            o.set("kind", "Imported")?;
            o.set("snapshots", *snapshots as f64)?;
        }
        _ => o.set("kind", "Unknown")?,
    }
    Ok(o)
}

/// `setMergeDriver` / `clearMergeDrivers`: swap the core handle for one built
/// with the new rules.
pub struct Rebuild {
    ticket: Ticket,
    /// `None` clears the rules.
    add: Option<(String, CoreResult<Rule>)>,
}

impl Task for Rebuild {
    type Output = CoreResult<()>;
    type JsValue = ();

    fn compute(&mut self) -> Result<Self::Output> {
        let mut guard = match self.ticket.lock()? {
            Ok(guard) => guard,
            Err(refused) => return Ok(Err(refused)),
        };
        let hooks = &self.ticket.hooks;
        let built = match self.add.take() {
            None => Ok((Drivers::new(), Vec::new())),
            Some((pattern, rule)) => {
                rule.and_then(|rule| hooks.drivers_with(Some((pattern, rule))))
            }
        };
        let (drivers, list) = match built {
            Ok(built) => built,
            Err(e) => return Ok(Err(e)),
        };
        // The builders consume the handle, so the new one is opened from the
        // same root with the same listener, and the old one is dropped.
        let fresh = velo_core::Repo::open_and_migrate(&hooks.root)
            .map(|r| r.listening(hooks.listener()).merging(drivers));
        Ok(fresh.map(|fresh| {
            *guard = fresh;
            hooks.set_rules(list);
        }))
    }

    fn resolve(&mut self, env: NapiEnv, output: Self::Output) -> Result<Self::JsValue> {
        output.map_err(|e| to_js(&env, e))
    }
}

#[napi]
impl Repo {
    /// Call `callback(event)` for every change committed through this handle.
    ///
    /// `event.kind` is `'Saved'`, `'Merged'`, `'RefMoved'` or `'Imported'`,
    /// next to that event's fields. Callbacks run when the promise of the call
    /// that made the change resolves: after the core call has finished and
    /// released the repository, on the JS thread. Events are in-process only:
    /// this handle hears its own writes, not those of another handle or process.
    #[napi(
        ts_args_type = "callback: (event: { kind: string; [field: string]: string | number | null }) => void"
    )]
    pub fn on_event(
        &self,
        env: &Env,
        callback: Function<'_, Object<'_>, Unknown<'_>>,
    ) -> Result<()> {
        self.hooks.add_callback(event_callback(env, callback)?);
        Ok(())
    }

    /// Merge files matching glob `pattern` with `driver`: `'json'`, `'yaml'`,
    /// `'toml'`, or a function `(ancestor, ours, theirs) => string | null`
    /// where `null` is a conflict. The first matching rule wins.
    ///
    /// A function runs inside the merge, under the repository lock, and must
    /// answer synchronously. Any call it makes on this handle rejects with
    /// `code: 'Reentrant'`. A throw (or a return that is not a string or
    /// `null`) makes `mergePlan` / `mergeCommit` reject with
    /// `code: 'DriverFailed'`, and nothing is committed.
    #[napi(
        ts_args_type = "pattern: string, driver: 'json' | 'yaml' | 'toml' | ((ancestor: string, ours: string, theirs: string) => string | null)",
        ts_return_type = "Promise<void>"
    )]
    pub fn set_merge_driver(
        &self,
        env: &Env,
        pattern: String,
        driver: Either<String, Function<'_, Args, Unknown<'_>>>,
    ) -> Result<AsyncTask<Rebuild>> {
        let rule = match driver {
            Either::A(name) => builtin(&name),
            Either::B(function) => Ok(self.hooks.custom_rule(env, function)?),
        };
        Ok(AsyncTask::new(Rebuild {
            ticket: self.ticket(),
            add: Some((pattern, rule)),
        }))
    }

    /// Go back to line-based merging for every path.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn clear_merge_drivers(&self) -> AsyncTask<Rebuild> {
        AsyncTask::new(Rebuild {
            ticket: self.ticket(),
            add: None,
        })
    }
}
