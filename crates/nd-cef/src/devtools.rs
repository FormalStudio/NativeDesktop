//! The DevTools protocol session each page keeps with its own browser: script evaluation in
//! named worlds, user scripts, `postMessage` bindings and the user agent. No front-end is
//! involved; messages go through `send_dev_tools_message` and come back to the observer.
//!
//! Site isolation runs a cross-site iframe in a renderer of its own, which the page's session
//! does not reach. Those frames are auto-attached as flat sessions on the same channel (the
//! message's `sessionId`, empty for the page's own), and each gets the page's scripts and
//! bindings before it runs.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use cef::*;
use serde_json::{Value, json};

use super::page;

type Reply = Box<dyn FnOnce(Result<Value, String>)>;

/// The binding pages post through; `window.webkit.messageHandlers.<name>.postMessage` calls it.
pub(crate) const BINDING: &str = "__ndPost";

/// The page's own session.
const PAGE: &str = "";

/// What the page's observer acts on.
pub(crate) enum Event {
    Binding(Binding),
    /// An out-of-process frame's session, paused until [`Devtools::prepare`] runs it.
    Attached(String),
}

/// A post through [`BINDING`].
pub(crate) struct Binding {
    /// The world it came from; empty for the page's own.
    pub world: String,
    pub origin: String,
    /// The posting frame's DevTools frame id.
    pub frame: String,
    pub main_frame: bool,
    pub payload: String,
}

#[derive(Clone, Debug, PartialEq)]
struct Context {
    frame: String,
    /// The world's name; empty or the page's for its default context.
    name: String,
    default: bool,
    origin: String,
}

/// Execution contexts by session and id: ids are only unique within one renderer.
#[derive(Default)]
struct Contexts(HashMap<(String, i64), Context>);

impl Contexts {
    fn created(&mut self, session: &str, context: &Value) {
        let (Some(id), Some(frame)) = (context["id"].as_i64(), context["auxData"]["frameId"].as_str()) else { return };
        self.0.insert(
            (session.to_owned(), id),
            Context {
                frame: frame.to_owned(),
                name: context["name"].as_str().unwrap_or_default().to_owned(),
                default: context["auxData"]["isDefault"].as_bool().unwrap_or(false),
                origin: context["origin"].as_str().unwrap_or_default().to_owned(),
            },
        );
    }

    fn destroyed(&mut self, session: &str, id: i64) {
        self.0.remove(&(session.to_owned(), id));
    }

    fn cleared(&mut self, session: &str) {
        self.0.retain(|(s, _), _| s != session);
    }

    fn get(&self, session: &str, id: i64) -> Option<&Context> {
        self.0.get(&(session.to_owned(), id))
    }

    /// `frame`'s context for `world` (its default one when empty).
    fn find(&self, frame: &str, world: &str) -> Option<(String, i64)> {
        self.0
            .iter()
            .find(|(_, c)| c.frame == frame && if world.is_empty() { c.default } else { !c.default && c.name == world })
            .map(|((s, id), _)| (s.clone(), *id))
    }

    /// The session whose renderer runs `frame`.
    fn session_of(&self, frame: &str) -> Option<String> {
        self.0.iter().find(|(_, c)| c.frame == frame).map(|((s, _), _)| s.clone())
    }
}

/// A script registered for every new document, with its identifier on the page's session and
/// on each frame session it was replayed to.
struct Script {
    identifier: String,
    source: String,
    world: Option<String>,
    sessions: HashMap<String, String>,
}

/// An out-of-process frame's session: its target id is the frame's id.
struct Session {
    frame: String,
}

#[derive(Default)]
pub(crate) struct Devtools {
    next: Cell<i32>,
    pending: RefCell<HashMap<i32, Reply>>,
    registration: RefCell<Option<Registration>>,
    host: RefCell<Option<BrowserHost>>,
    contexts: RefCell<Contexts>,
    main_frame: RefCell<String>,
    /// Bindings added, by world ("" for the page's).
    bound: RefCell<Vec<String>>,
    scripts: RefCell<Vec<Script>>,
    sessions: RefCell<HashMap<String, Session>>,
    page: Cell<u64>,
    /// Lookups waiting for the main frame's id.
    waiting: RefCell<Vec<Box<dyn FnOnce()>>>,
}

impl Devtools {
    pub(crate) fn attach(&self, id: u64, host: &BrowserHost) {
        let mut observer = NdObserver::new(id);
        *self.registration.borrow_mut() = host.add_dev_tools_message_observer(Some(&mut observer));
        *self.host.borrow_mut() = Some(host.clone());
        self.page.set(id);
        self.call("Runtime.enable", json!({}), |_| {});
        self.call("Page.enable", json!({}), |_| {});
        self.auto_attach(PAGE);
        self.call("Page.getFrameTree", json!({}), move |r| {
            if let (Ok(tree), Some(page)) = (r, page::get(id))
                && let Some(frame) = tree["frameTree"]["frame"]["id"].as_str()
            {
                page.devtools.set_main_frame(frame);
            }
        });
    }

    pub(crate) fn detach(&self) {
        self.registration.borrow_mut().take();
        self.host.borrow_mut().take();
        for (_, reply) in self.pending.borrow_mut().drain() {
            reply(Err("the page closed".into()));
        }
    }

    /// Sends `method` on the page's session and hands its result (or error message) to `reply`.
    pub(crate) fn call(&self, method: &str, params: Value, reply: impl FnOnce(Result<Value, String>) + 'static) {
        self.call_on(PAGE, method, params, reply);
    }

    /// [`Self::call`] on a frame's `session` (the page's when empty).
    fn call_on(&self, session: &str, method: &str, params: Value, reply: impl FnOnce(Result<Value, String>) + 'static) {
        let Some(host) = self.host.borrow().clone() else {
            reply(Err("the page has no browser yet".into()));
            return;
        };
        let id = self.next.get() + 1;
        self.next.set(id);
        self.pending.borrow_mut().insert(id, Box::new(reply));
        let mut message = json!({ "id": id, "method": method, "params": params });
        if !session.is_empty() {
            message["sessionId"] = json!(session);
        }
        if host.send_dev_tools_message(Some(message.to_string().as_bytes())) != 1
            && let Some(reply) = self.pending.borrow_mut().remove(&id)
        {
            reply(Err(format!("{method} was refused")));
        }
    }

    /// A raw protocol message: a method's result, or an event on `sessionId`.
    pub(crate) fn message(&self, message: &Value) -> Option<Event> {
        if let Some(id) = message["id"].as_i64() {
            let reply = self.pending.borrow_mut().remove(&(id as i32))?;
            match message.get("error") {
                Some(error) => reply(Err(error["message"].as_str().map_or_else(|| error.to_string(), str::to_owned))),
                None => reply(Ok(message["result"].clone())),
            }
            return None;
        }
        let method = message["method"].as_str()?;
        self.event(message["sessionId"].as_str().unwrap_or(PAGE), method, &message["params"])
    }

    /// Protocol events on `session`.
    fn event(&self, session: &str, method: &str, params: &Value) -> Option<Event> {
        match method {
            "Runtime.executionContextCreated" => self.contexts.borrow_mut().created(session, &params["context"]),
            "Runtime.executionContextDestroyed" => {
                if let Some(id) = params["executionContextId"].as_i64() {
                    self.contexts.borrow_mut().destroyed(session, id);
                }
            }
            "Runtime.executionContextsCleared" => self.contexts.borrow_mut().cleared(session),
            "Page.frameNavigated" if session.is_empty() => {
                let frame = &params["frame"];
                if frame["parentId"].is_null()
                    && let Some(id) = frame["id"].as_str()
                {
                    self.set_main_frame(id);
                }
            }
            "Target.attachedToTarget" => {
                let (Some(child), Some(target)) =
                    (params["sessionId"].as_str(), params["targetInfo"]["targetId"].as_str())
                else {
                    return None;
                };
                self.sessions.borrow_mut().insert(child.to_owned(), Session { frame: target.to_owned() });
                return Some(Event::Attached(child.to_owned()));
            }
            "Target.detachedFromTarget" => {
                if let Some(child) = params["sessionId"].as_str() {
                    self.forget(child);
                }
            }
            "Runtime.bindingCalled" if params["name"].as_str() == Some(BINDING) => {
                let context = params["executionContextId"].as_i64().unwrap_or_default();
                let contexts = self.contexts.borrow();
                let c = contexts.get(session, context)?;
                return Some(Event::Binding(Binding {
                    world: if c.default { String::new() } else { c.name.clone() },
                    origin: c.origin.clone(),
                    main_frame: c.frame == *self.main_frame.borrow(),
                    frame: c.frame.clone(),
                    payload: params["payload"].as_str().unwrap_or_default().to_owned(),
                }));
            }
            _ => {}
        }
        None
    }

    /// Attaches the out-of-process frames under `session` paused, so each takes the page's
    /// scripts before its document starts.
    fn auto_attach(&self, session: &str) {
        let params = json!({
            "autoAttach": true,
            "waitForDebuggerOnStart": true,
            "flatten": true,
            "filter": [{ "type": "iframe" }],
        });
        self.call_on(session, "Target.setAutoAttach", params, |_| {});
    }

    /// A new frame session: the page's scripts and bindings, then its own frames, then it runs.
    pub(crate) fn prepare(&self, session: &str) {
        self.call_on(session, "Runtime.enable", json!({}), |_| {});
        self.call_on(session, "Page.enable", json!({}), |_| {});
        let scripts: Vec<_> =
            self.scripts.borrow().iter().map(|s| (s.identifier.clone(), s.source.clone(), s.world.clone())).collect();
        for (identifier, source, world) in scripts {
            self.replay(session, &identifier, &source, world.as_deref());
        }
        for world in self.bound.borrow().clone() {
            self.call_on(session, "Runtime.addBinding", binding_params(&world), |_| {});
        }
        self.auto_attach(session);
        self.call_on(session, "Runtime.runIfWaitingForDebugger", json!({}), |_| {});
    }

    fn forget(&self, session: &str) {
        self.sessions.borrow_mut().remove(session);
        self.contexts.borrow_mut().cleared(session);
        for script in self.scripts.borrow_mut().iter_mut() {
            script.sessions.remove(session);
        }
    }

    /// Registers the page's script `identifier` on a frame `session`.
    fn replay(&self, session: &str, identifier: &str, source: &str, world: Option<&str>) {
        let (id, session, identifier) = (self.page.get(), session.to_owned(), identifier.to_owned());
        self.call_on(
            &session.clone(),
            "Page.addScriptToEvaluateOnNewDocument",
            script_params(source, world),
            move |r| {
                let (Some(page), Some(added)) =
                    (page::get(id), r.ok().and_then(|v| v["identifier"].as_str().map(str::to_owned)))
                else {
                    return;
                };
                let d = &page.devtools;
                let live = d.sessions.borrow().contains_key(&session);
                let kept = live
                    && d.scripts
                        .borrow_mut()
                        .iter_mut()
                        .find(|s| s.identifier == identifier)
                        .map(|s| s.sessions.insert(session.clone(), added.clone()))
                        .is_some();
                if live && !kept {
                    d.call_on(
                        &session,
                        "Page.removeScriptToEvaluateOnNewDocument",
                        json!({ "identifier": added }),
                        |_| {},
                    );
                }
            },
        );
    }

    fn set_main_frame(&self, id: &str) {
        *self.main_frame.borrow_mut() = id.to_owned();
        let waiting = std::mem::take(&mut *self.waiting.borrow_mut());
        for f in waiting {
            f();
        }
    }

    /// Where code for `world` runs in `frame` (the main frame when `None`): the session and the
    /// context, the isolated world created when the frame has none yet. The main frame's page
    /// world needs no context id.
    fn context_for(
        &self,
        frame: Option<&str>,
        world: &str,
        then: impl FnOnce(Result<(String, Option<i64>), String>) + 'static,
    ) {
        if frame.is_none() && world.is_empty() {
            then(Ok((PAGE.to_owned(), None)));
            return;
        }
        if frame.is_none() && self.main_frame.borrow().is_empty() {
            let (id, world) = (self.page.get(), world.to_owned());
            self.waiting.borrow_mut().push(Box::new(move || match page::get(id) {
                Some(p) => p.devtools.context_for(None, &world, then),
                None => then(Err("the page closed".into())),
            }));
            return;
        }
        let frame = frame.map_or_else(|| self.main_frame.borrow().clone(), str::to_owned);
        if let Some((session, id)) = self.contexts.borrow().find(&frame, world) {
            then(Ok((session, Some(id))));
            return;
        }
        let session = self.contexts.borrow().session_of(&frame).or_else(|| {
            self.sessions.borrow().iter().find(|(_, s)| s.frame == frame).map(|(session, _)| session.clone())
        });
        let session = match session {
            Some(session) => session,
            None if frame == *self.main_frame.borrow() => PAGE.to_owned(),
            None => {
                then(Err(format!("no frame {frame} in the page")));
                return;
            }
        };
        if world.is_empty() {
            then(Err(format!("frame {frame} has no document yet")));
            return;
        }
        let params = json!({ "frameId": frame, "worldName": world });
        self.call_on(&session.clone(), "Page.createIsolatedWorld", params, move |r| {
            then(r.map(|v| (session, v["executionContextId"].as_i64())));
        });
    }

    /// Evaluates the framework's own `code` and answers with its completion value: a thrown
    /// exception is an error, a promise is not awaited. It runs without a user gesture, so
    /// nothing the framework runs hands the page user activation (popups, fullscreen,
    /// picture-in-picture) it did not get from the user.
    pub(crate) fn evaluate(&self, code: &str, world: Option<&str>, done: impl FnOnce(Result<Value, String>) + 'static) {
        self.run(code, world, None, false, done);
    }

    /// [`Self::evaluate`] for the app's `execute_javascript`, in `frame` when given. When the
    /// app acts for the user (a shortcut, a menu item) the code runs with a user gesture, as on
    /// WebKit, and may open windows, enter fullscreen or picture-in-picture.
    pub(crate) fn evaluate_for_app(
        &self,
        code: &str,
        world: Option<&str>,
        frame: Option<&str>,
        gesture: nd_webview::UserGesture,
        done: impl FnOnce(Result<Value, String>) + 'static,
    ) {
        self.run(code, world, frame, gesture == nd_webview::UserGesture::With, done);
    }

    fn run(
        &self,
        code: &str,
        world: Option<&str>,
        frame: Option<&str>,
        gesture: bool,
        done: impl FnOnce(Result<Value, String>) + 'static,
    ) {
        let code = code.to_owned();
        let id = self.page.get();
        self.context_for(frame, world.unwrap_or_default(), move |target| {
            let Some(page) = page::get(id) else {
                done(Err("the page closed".into()));
                return;
            };
            let (session, context) = match target {
                Ok(target) => target,
                Err(e) => {
                    done(Err(e));
                    return;
                }
            };
            let mut params =
                json!({ "expression": code, "returnByValue": true, "awaitPromise": false, "userGesture": gesture });
            if let Some(context) = context {
                params["contextId"] = json!(context);
            }
            page.devtools.call_on(&session, "Runtime.evaluate", params, move |r| {
                done(r.and_then(|v| {
                    if let Some(ex) = v.get("exceptionDetails") {
                        let message = ex["exception"]["description"]
                            .as_str()
                            .or(ex["text"].as_str())
                            .unwrap_or("the script threw")
                            .to_owned();
                        return Err(message);
                    }
                    Ok(v["result"]["value"].clone())
                }))
            });
        });
    }

    /// Registers `source` for every new document of the page and its out-of-process frames
    /// (in `world` when given) and answers with the identifier that removes it.
    pub(crate) fn add_script(&self, source: &str, world: Option<&str>, done: impl FnOnce(Option<String>) + 'static) {
        let (id, source, world) =
            (self.page.get(), source.to_owned(), world.filter(|w| !w.is_empty()).map(str::to_owned));
        self.call("Page.addScriptToEvaluateOnNewDocument", script_params(&source, world.as_deref()), move |r| {
            let identifier = r.ok().and_then(|v| v["identifier"].as_str().map(str::to_owned));
            if let (Some(identifier), Some(page)) = (&identifier, page::get(id)) {
                let d = &page.devtools;
                d.scripts.borrow_mut().push(Script {
                    identifier: identifier.clone(),
                    source: source.clone(),
                    world: world.clone(),
                    sessions: HashMap::new(),
                });
                let sessions: Vec<String> = d.sessions.borrow().keys().cloned().collect();
                for session in sessions {
                    d.replay(&session, identifier, &source, world.as_deref());
                }
            }
            done(identifier);
        });
    }

    pub(crate) fn remove_script(&self, identifier: &str) {
        let script = {
            let mut scripts = self.scripts.borrow_mut();
            scripts.iter().position(|s| s.identifier == identifier).map(|i| scripts.remove(i))
        };
        for (session, identifier) in script.map(|s| s.sessions).unwrap_or_default() {
            self.call_on(
                &session,
                "Page.removeScriptToEvaluateOnNewDocument",
                json!({ "identifier": identifier }),
                |_| {},
            );
        }
        self.call("Page.removeScriptToEvaluateOnNewDocument", json!({ "identifier": identifier }), |_| {});
    }

    /// Exposes [`BINDING`] to `world` (the page's when empty) in the page and its frames, once
    /// per world.
    pub(crate) fn bind(&self, world: &str) {
        if self.bound.borrow().iter().any(|w| w == world) {
            return;
        }
        self.bound.borrow_mut().push(world.to_owned());
        let sessions: Vec<String> = self.sessions.borrow().keys().cloned().collect();
        for session in std::iter::once(PAGE.to_owned()).chain(sessions) {
            self.call_on(&session, "Runtime.addBinding", binding_params(world), |_| {});
        }
    }
}

fn script_params(source: &str, world: Option<&str>) -> Value {
    let mut params = json!({ "source": source });
    if let Some(world) = world.filter(|w| !w.is_empty()) {
        params["worldName"] = json!(world);
    }
    params
}

fn binding_params(world: &str) -> Value {
    let mut params = json!({ "name": BINDING });
    if !world.is_empty() {
        params["executionContextName"] = json!(world);
    }
    params
}

/// The page-side shim behind `window.webkit.messageHandlers.<name>.postMessage(body)`.
pub(crate) fn handler_shim(name: &str) -> String {
    let name = serde_json::to_string(name).unwrap_or_else(|_| "\"\"".into());
    format!(
        "(function(){{var n={name};var w=window.webkit=window.webkit||{{}};var h=w.messageHandlers=w.messageHandlers||{{}};\
         h[n]={{postMessage:function(b){{{BINDING}(JSON.stringify({{name:n,body:b===undefined?null:b}}));}}}};}})();"
    )
}

pub(crate) fn remove_shim(name: &str) -> String {
    let name = serde_json::to_string(name).unwrap_or_else(|_| "\"\"".into());
    format!("(function(){{var w=window.webkit;if(w&&w.messageHandlers)delete w.messageHandlers[{name}];}})();")
}

/// A user script's source for every document CDP runs it in: only the top frame unless
/// `all_frames`, and after parsing for [`nd_webview::Injection::End`].
pub(crate) fn script_source(script: &nd_webview::UserScript) -> String {
    let mut source = script.guarded_source();
    if script.injection == nd_webview::Injection::End {
        source = format!(
            "(function(){{var r=function(){{\n{source}\n}};if(document.readyState==='loading')\
             document.addEventListener('DOMContentLoaded',r,{{once:true}});else r();}})();"
        );
    }
    if !script.all_frames {
        source = format!("if(window.top===window){{\n{source}\n}}");
    }
    source
}

wrap_task! {
    struct Prepare {
        page: u64,
        session: String,
    }

    impl Task {
        fn execute(&self) {
            if let Some(page) = page::get(self.page)
                && page.devtools.sessions.borrow().contains_key(&self.session)
            {
                page.devtools.prepare(&self.session);
            }
        }
    }
}

wrap_dev_tools_message_observer! {
    struct NdObserver {
        id: u64,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_message(&self, _browser: Option<&mut Browser>, message: Option<&[u8]>) -> i32 {
            let Some(page) = page::get(self.id) else { return 0 };
            let Some(message) = message.and_then(|m| serde_json::from_slice::<Value>(m).ok()) else { return 0 };
            match page.devtools.message(&message) {
                Some(Event::Binding(b)) => page.script_message(b),
                // CEF hands the attach over while Chromium is still attaching, before the session
                // listens for its resume: a resume sent from here leaves the frame paused.
                Some(Event::Attached(session)) => {
                    post_task(ThreadId::UI, Some(&mut Prepare::new(self.id, session)));
                }
                None => {}
            }
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use nd_webview::UserScript;

    #[test]
    fn end_scripts_wait_for_parsing_and_stay_in_the_top_frame() {
        let s = script_source(&UserScript::new("a", "go()"));
        assert!(s.starts_with("if(window.top===window){"), "{s}");
        assert!(s.contains("DOMContentLoaded"), "{s}");
        let all = script_source(&UserScript::new("a", "go()").at_start().all_frames());
        assert_eq!(all, "go()");
    }

    fn created(d: &Devtools, session: &str, id: i64, frame: &str, name: &str, default: bool) {
        let context = json!({ "id": id, "name": name, "origin": format!("http://{frame}"),
            "auxData": { "frameId": frame, "isDefault": default } });
        let event = json!({ "method": "Runtime.executionContextCreated", "params": { "context": context } });
        let mut event = event;
        if !session.is_empty() {
            event["sessionId"] = json!(session);
        }
        assert!(d.message(&event).is_none());
    }

    fn post(d: &Devtools, session: &str, context: i64) -> Option<Binding> {
        let params = json!({ "name": BINDING, "executionContextId": context, "payload": "{}" });
        match d.message(&json!({ "method": "Runtime.bindingCalled", "params": params, "sessionId": session })) {
            Some(Event::Binding(b)) => Some(b),
            _ => None,
        }
    }

    fn attached(d: &Devtools, session: &str) {
        let info = json!({ "targetId": session.to_uppercase(), "type": "iframe" });
        let event = d.message(
            &json!({ "method": "Target.attachedToTarget", "params": { "sessionId": session, "targetInfo": info } }),
        );
        assert!(matches!(event, Some(Event::Attached(s)) if s == session));
    }

    #[test]
    fn bindings_name_the_frame_of_their_own_session() {
        let d = Devtools::default();
        d.set_main_frame("MAIN");
        attached(&d, "s1");
        created(&d, "", 1, "MAIN", "", true);
        created(&d, "s1", 1, "S1", "nd", false);
        let main = post(&d, "", 1).expect("main post");
        assert!(main.main_frame && main.frame == "MAIN" && main.world.is_empty());
        let sub = post(&d, "s1", 1).expect("iframe post");
        assert!(!sub.main_frame && sub.frame == "S1" && sub.world == "nd" && sub.origin == "http://S1");
        assert!(post(&d, "s2", 1).is_none());
        d.message(&json!({ "method": "Target.detachedFromTarget", "params": { "sessionId": "s1" } }));
        assert!(post(&d, "s1", 1).is_none());
        assert!(d.sessions.borrow().is_empty());
        assert!(post(&d, "", 1).is_some());
    }

    #[test]
    fn frames_resolve_to_their_session_and_world() {
        let d = Devtools::default();
        d.set_main_frame("MAIN");
        attached(&d, "s1");
        created(&d, "s1", 7, "S1", "", true);
        created(&d, "s1", 8, "S1", "nd", false);
        created(&d, "", 7, "SAME", "", true);
        let found = Rc::new(RefCell::new(Vec::new()));
        for (frame, world) in [(Some("S1"), ""), (Some("S1"), "nd"), (Some("SAME"), ""), (None, ""), (Some("GONE"), "")]
        {
            let found = found.clone();
            d.context_for(frame, world, move |r| found.borrow_mut().push(r));
        }
        let found = found.borrow();
        assert_eq!(found[0], Ok(("s1".into(), Some(7))));
        assert_eq!(found[1], Ok(("s1".into(), Some(8))));
        assert_eq!(found[2], Ok((String::new(), Some(7))));
        assert_eq!(found[3], Ok((String::new(), None)));
        assert!(found[4].is_err());
    }

    #[test]
    fn results_reach_their_caller() {
        let d = Devtools::default();
        let got = Rc::new(RefCell::new(None));
        let g = got.clone();
        d.pending.borrow_mut().insert(3, Box::new(move |r| *g.borrow_mut() = Some(r)));
        d.message(&json!({ "id": 3, "error": { "message": "no" }, "sessionId": "s1" }));
        assert_eq!(*got.borrow(), Some(Err("no".to_owned())));
    }

    #[test]
    fn shims_post_through_the_binding() {
        let s = handler_shim("bridge");
        assert!(s.contains(r#"var n="bridge""#), "{s}");
        assert!(s.contains(BINDING), "{s}");
    }
}
