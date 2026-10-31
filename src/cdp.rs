//! Minimal Chrome DevTools Protocol client: enough to observe a live page and act on it
//! by element identity rather than by pixel coordinates.
//!
//! Observation goes through the page's own DOM rather than through the accessibility
//! tree. That choice is deliberate: the DOM carries the `href` a step needs in order to
//! still be recognised on a page whose contents have changed, and it lets every element
//! be handed a handle we can act on without recomputing coordinates at all.

use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

pub struct Cdp {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

/// One addressable thing on screen: what it is, what it says, where it points, and the
/// handle that lets us act on it.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Element {
    pub handle: String,
    pub tag: String,
    pub role: String,
    pub name: String,
    pub href: String,
}

const EXTRACT: &str = r#"(() => {
  document.querySelectorAll('[data-mint-id]').forEach(e => e.removeAttribute('data-mint-id'));
  const ACT = 'a,button,input,textarea,select,[role="button"],[role="link"],[role="tab"],[role="menuitem"],[role="searchbox"],[role="combobox"],[role="checkbox"]';
  const roleOf = (el) => {
    const r = el.getAttribute('role');
    if (r) return r;
    const t = el.tagName.toLowerCase();
    if (t === 'a') return 'link';
    if (t === 'button') return 'button';
    if (t === 'input') {
      const k = (el.getAttribute('type') || 'text').toLowerCase();
      if (k === 'search') return 'searchbox';
      if (k === 'submit' || k === 'button') return 'button';
      return 'textbox';
    }
    if (t === 'textarea') return 'textbox';
    if (t === 'select') return 'combobox';
    return t;
  };
  const out = [], seen = new Set();
  for (const el of document.querySelectorAll(ACT)) {
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) continue;
    const st = getComputedStyle(el);
    if (st.visibility === 'hidden' || st.display === 'none' || st.opacity === '0') continue;
    const text = (el.innerText || el.value || el.getAttribute('aria-label') || el.getAttribute('title') || '')
      .trim().replace(/\s+/g, ' ');
    const href = el.getAttribute('href') || '';
    const role = roleOf(el);
    // An empty field has no inner text and no value, so a name has to be found elsewhere or the
    // field is invisible to the runtime — and a runtime that cannot see an empty field can never
    // learn to fill one. The placeholder, the label it belongs to, and its name attribute are
    // the three places that survive.
    let name = text;
    if (!name) {
      const id = el.getAttribute('id');
      let fromLabel = '';
      if (id) {
        const l = document.querySelector('label[for="' + CSS.escape(id) + '"]');
        if (l) fromLabel = l.innerText || '';
      }
      if (!fromLabel) {
        const up = el.closest('label');
        if (up) fromLabel = up.innerText || '';
      }
      name = (el.getAttribute('placeholder') || fromLabel || el.getAttribute('name') || el.getAttribute('id') || '')
        .trim().replace(/\s+/g, ' ');
    }
    if (!name && !href) continue;
    const key = role + '|' + name + '|' + href;
    if (seen.has(key)) continue;
    seen.add(key);
    const id = 'j' + out.length;
    el.setAttribute('data-mint-id', id);
    out.push({ handle: id, tag: el.tagName.toLowerCase(), role: role,
               name: name.slice(0, 140), href: href.slice(0, 300) });
    if (out.length >= 250) break;
  }
  return JSON.stringify({ url: location.href, title: document.title, els: out });
})()"#;

impl Cdp {
    /// Connect to an already-known target address.
    pub async fn connect_ws(ws_url: &str) -> Result<Self> {
        let (ws, _) = connect_async(ws_url).await.context("websocket refused")?;
        let mut cdp = Cdp { ws, next_id: 1 };
        for method in ["Page.enable", "Runtime.enable"] {
            cdp.call(method, json!({})).await?;
        }
        Ok(cdp)
    }

    /// Open a second tab and hand back its address. Two tabs in one browser is what lets
    /// the demo show both runs side by side without two browsers fighting over a screen.
    pub async fn open_tab(port: u16, url: &str) -> Result<String> {
        let endpoint = format!("http://127.0.0.1:{}/json/new?{}", port, url);
        let v: Value = match ureq::put(&endpoint).call() {
            Ok(r) => r.into_json().context("new tab returned junk")?,
            Err(_) => ureq::get(&endpoint).call().context("cannot open a second tab")?.into_json()?,
        };
        v.get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("new tab has no address"))
    }

    /// Close every page tab except one, so a run starts from a known tab instead of whatever
    /// the last run left behind.
    ///
    /// Tabs accumulate: every run opens one for the compiled side and nothing closes it. After
    /// enough runs the runtime attaches to a stale tab and a working run looks broken for no
    /// visible reason — which is exactly what happened before this existed.
    pub async fn tidy_tabs(port: u16) -> Result<usize> {
        let list: Value = ureq::get(&format!("http://127.0.0.1:{}/json", port))
            .call()
            .context("cannot reach the browser debug port")?
            .into_json()
            .context("bad target list")?;
        let ids: Vec<String> = list
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|t| t.get("type").and_then(Value::as_str) == Some("page"))
                    .filter_map(|t| t.get("id").and_then(Value::as_str).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let mut closed = 0;
        for id in ids.iter().skip(1) {
            if ureq::get(&format!("http://127.0.0.1:{}/json/close/{}", port, id))
                .call()
                .is_ok()
            {
                closed += 1;
            }
        }
        // Never leave the browser with no page at all: a run then has nothing to attach to,
        // and the failure looks like a broken run rather than an empty browser.
        if ids.len() <= 1 {
            let _ = ureq::put(&format!("http://127.0.0.1:{}/json/new?about:blank", port)).call();
        }
        Ok(closed)
    }

    pub async fn connect(port: u16) -> Result<Self> {
        let list: Value = ureq::get(&format!("http://127.0.0.1:{}/json", port))
            .call()
            .context("cannot reach the browser debug port")?
            .into_json()
            .context("bad target list")?;
        let target = list
            .as_array()
            .and_then(|a| a.iter().find(|t| t.get("type").and_then(Value::as_str) == Some("page")))
            .ok_or_else(|| anyhow!("no page target"))?;
        let ws_url = target
            .get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("target has no websocket url"))?
            .to_string();
        let (ws, _) = connect_async(&ws_url).await.context("websocket refused")?;
        let mut cdp = Cdp { ws, next_id: 1 };
        for method in ["Page.enable", "Runtime.enable"] {
            cdp.call(method, json!({})).await?;
        }
        Ok(cdp)
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({ "id": id, "method": method, "params": params });
        self.ws.send(Message::Text(msg.to_string())).await?;
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(30), self.ws.next())
                .await
                .map_err(|_| anyhow!("{method}: timed out"))?
                .ok_or_else(|| anyhow!("{method}: socket closed"))??;
            let text = match frame {
                Message::Text(t) => t.to_string(),
                Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
                Message::Close(_) => return Err(anyhow!("{method}: browser closed")),
                _ => continue,
            };
            let v: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(err) = v.get("error") {
                    return Err(anyhow!("{method}: {err}"));
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }

    async fn eval(&mut self, expression: &str) -> Result<Value> {
        let r = self
            .call(
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
            )
            .await?;
        if let Some(details) = r.pointer("/exceptionDetails") {
            return Err(anyhow!("page script failed: {details}"));
        }
        Ok(r.pointer("/result/value").cloned().unwrap_or(Value::Null))
    }

    /// Wait until the page stops changing: two consecutive observations of the same
    /// screen. Far more reliable than sleeping for a guessed number of milliseconds.
    pub async fn settle(&mut self, max_ms: u64) -> Result<()> {
        let deadline = Instant::now() + Duration::from_millis(max_ms);
        let mut last: Option<u64> = None;
        while Instant::now() < deadline {
            let (els, url) = self.state().await?;
            let sig = crate::state::signature(&url, &els);
            if last == Some(sig) {
                return Ok(());
            }
            last = Some(sig);
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
        Ok(())
    }

    pub async fn navigate(&mut self, url: &str) -> Result<u64> {
        let started = Instant::now();
        self.call("Page.navigate", json!({ "url": url })).await?;
        for _ in 0..160 {
            let ready = self.eval("document.readyState").await.ok();
            if ready.as_ref().and_then(Value::as_str) == Some("complete") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(started.elapsed().as_micros() as u64)
    }

    /// The page as a compact list of things a person could act on, each with a handle.
    pub async fn state(&mut self) -> Result<(Vec<Element>, String)> {
        let raw = self.eval(EXTRACT).await?;
        let s = raw.as_str().ok_or_else(|| anyhow!("state extraction returned nothing"))?;
        let v: Value = serde_json::from_str(s)?;
        let url = v.get("url").and_then(Value::as_str).unwrap_or("").to_string();
        let els: Vec<Element> = serde_json::from_value(v.get("els").cloned().unwrap_or(json!([])))?;
        Ok((els, url))
    }

    /// Act by handle. The handle was stamped on the element when we observed it, so there
    /// are no coordinates to compute and nothing to mis-aim.
    pub async fn click_handle(&mut self, handle: &str) -> Result<bool> {
        let js = format!(
            "(() => {{ const el = document.querySelector('[data-mint-id=\"{handle}\"]'); \
             if (!el) return 'gone'; el.scrollIntoView({{block:'center'}}); el.click(); return 'ok'; }})()"
        );
        Ok(self.eval(&js).await?.as_str() == Some("ok"))
    }

    /// Type into a field by handle, through the page's own events so that frameworks with
    /// controlled inputs see a real keystroke.
    pub async fn type_handle(&mut self, handle: &str, text: &str) -> Result<bool> {
        let js = format!(
            "(() => {{ const el = document.querySelector('[data-mint-id=\"{handle}\"]'); \
             if (!el) return 'gone'; el.focus(); \
             const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement : HTMLInputElement; \
             const desc = Object.getOwnPropertyDescriptor(proto.prototype, 'value'); \
             if (desc && desc.set) desc.set.call(el, {text_json}); else el.value = {text_json}; \
             el.dispatchEvent(new Event('input', {{bubbles:true}})); \
             el.dispatchEvent(new Event('change', {{bubbles:true}})); \
             if (el.form) {{ el.form.requestSubmit ? el.form.requestSubmit() : el.form.submit(); }} \
             return 'ok'; }})()",
            text_json = serde_json::to_string(text)?
        );
        Ok(self.eval(&js).await?.as_str() == Some("ok"))
    }

    pub async fn current_url(&mut self) -> Result<String> {
        Ok(self.eval("location.href").await?.as_str().unwrap_or("").to_string())
    }

    pub async fn title(&mut self) -> Result<String> {
        Ok(self.eval("document.title").await?.as_str().unwrap_or("").to_string())
    }

    /// One frame of the live page, as a data URL the browser can show directly.
    pub async fn screenshot(&mut self, quality: u8) -> Result<String> {
        let r = self
            .call("Page.captureScreenshot", json!({ "format": "jpeg", "quality": quality }))
            .await?;
        let data = r.get("data").and_then(Value::as_str).unwrap_or("");
        Ok(format!("data:image/jpeg;base64,{data}"))
    }

    /// The page's visible text — used to judge whether an outcome satisfies the goal.
    pub async fn text(&mut self) -> Result<String> {
        Ok(self.eval("document.body.innerText").await?.as_str().unwrap_or("").to_string())
    }

    /// What a naive agent would send instead of our compact state: the page's whole text
    /// and its whole accessibility tree. Measured, never assumed, because the argument
    /// for this design rests on the difference.
    pub async fn naive_payload(&mut self) -> Result<(usize, usize)> {
        let text_len = self.eval("document.body.innerText.length").await?.as_u64().unwrap_or(0) as usize;
        let ax = self.call("Accessibility.getFullAXTree", json!({ "depth": -1 })).await?;
        let nodes = ax.get("nodes").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
        let tree_len = serde_json::to_string(&ax).map(|s| s.len()).unwrap_or(0);
        Ok((text_len.max(tree_len), nodes))
    }
}
