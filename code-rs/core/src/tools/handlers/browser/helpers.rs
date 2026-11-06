use crate::codex::Session;
use serde_json::Value;
use std::sync::Arc;

/// Get the browser manager for the session (always uses global).
pub(super) async fn get_browser_manager_for_session(
    _sess: &Session,
) -> Option<Arc<code_browser::BrowserManager>> {
    // Always use the global browser manager.
    code_browser::global::get_browser_manager().await
}

pub(super) async fn resolve_target_id_from_value(
    browser_manager: &code_browser::BrowserManager,
    value: &Value,
) -> Result<String, String> {
    if let Some(target_id) = value.get("target_id").and_then(serde_json::Value::as_str) {
        let trimmed = target_id.trim();
        if trimmed.is_empty() {
            return Err("target_id must be non-empty".to_owned());
        }
        return Ok(trimmed.to_owned());
    }

    if let Some(index) = value.get("index").and_then(serde_json::Value::as_u64) {
        if index == 0 {
            return Err("index must be >= 1".to_owned());
        }
        let targets = browser_manager
            .list_page_targets()
            .await
            .map_err(|e| format!("Failed to list browser targets: {e}"))?;
        let idx = usize::try_from(index - 1)
            .map_err(|_| format!("index {index} too large"))?;
        match targets.get(idx) {
            Some(target) => Ok(target.target_id.clone()),
            None => Err(format!(
                "index out of range (got {index}, available {len})",
                len = targets.len()
            )),
        }
    } else {
        Err("Missing target_id or index".to_owned())
    }
}

pub(super) fn unwrap_execute_javascript_value(result: Value) -> Result<Value, String> {
    let Some(obj) = result.as_object() else {
        return Ok(result);
    };

    let success = obj
        .get("success")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if success {
        Ok(obj.get("value").cloned().unwrap_or(serde_json::Value::Null))
    } else {
        let err = obj
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("JavaScript execution failed");
        Err(err.to_owned())
    }
}

pub(super) async fn selector_rect_after_scroll(
    browser_manager: &code_browser::BrowserManager,
    selector: &str,
    focus: bool,
) -> Result<(f64, f64, f64, f64), String> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err("selector must be non-empty".to_owned());
    }

    let selector_json =
        serde_json::to_string(selector).map_err(|e| format!("Failed to encode selector: {e}"))?;
    let focus_js = if focus {
        "try{el.focus({preventScroll:true});}catch(_){try{el.focus();}catch(_){}}"
    } else {
        ""
    };

    let script = format!(
        r#"(function() {{
            try {{
                var sel = {selector_json};
                var el = document.querySelector(sel);
                if (!el) return {{ ok: false, error: "selector not found", selector: sel }};
                try {{
                    el.scrollIntoView({{ block: "center", inline: "center" }});
                }} catch (_) {{
                    try {{ el.scrollIntoView(); }} catch (_) {{}}
                }}
                {focus_js}
                var r = el.getBoundingClientRect();
                return {{
                    ok: true,
                    selector: sel,
                    rect: {{ x: r.x, y: r.y, width: r.width, height: r.height }},
                    scroll: {{ x: window.scrollX, y: window.scrollY }},
                }};
            }} catch (e) {{
                return {{ ok: false, error: String(e) }};
            }}
        }})()"#
    );

    let raw = browser_manager
        .execute_javascript(&script)
        .await
        .map_err(|e| format!("Failed to execute selector query: {e}"))?;
    let value = unwrap_execute_javascript_value(raw)?;

    if value
        .get("ok")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        let rect = value.get("rect").ok_or_else(|| "Missing rect".to_owned())?;
        let x = rect.get("x").and_then(serde_json::Value::as_f64)
            .ok_or_else(|| "Missing or invalid rect.x".to_owned())?;
        let y = rect.get("y").and_then(serde_json::Value::as_f64)
            .ok_or_else(|| "Missing or invalid rect.y".to_owned())?;
        let w = rect.get("width").and_then(serde_json::Value::as_f64)
            .ok_or_else(|| "Missing or invalid rect.width".to_owned())?;
        let h = rect.get("height").and_then(serde_json::Value::as_f64)
            .ok_or_else(|| "Missing or invalid rect.height".to_owned())?;
        Ok((x, y, w, h))
    } else {
        let error = value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("selector query failed");
        Err(error.to_owned())
    }
}

pub(super) async fn inspect_dom_target(
    browser_manager: &code_browser::BrowserManager,
    selector: Option<&str>,
    id: Option<&str>,
    coordinates: Option<(f64, f64)>,
) -> Result<Value, String> {
    let selector = selector.map(str::trim).filter(|value| !value.is_empty());
    let id = id.map(str::trim).filter(|value| !value.is_empty());
    let selector_json = serde_json::to_string(&selector)
        .map_err(|error| format!("Failed to encode selector: {error}"))?;
    let id_json =
        serde_json::to_string(&id).map_err(|error| format!("Failed to encode id: {error}"))?;
    let coordinates_json = serde_json::to_string(&coordinates)
        .map_err(|error| format!("Failed to encode coordinates: {error}"))?;
    let script = format!(
        r#"(() => {{
            try {{
                const selector = {selector_json};
                const id = {id_json};
                const coordinates = {coordinates_json};
                let element = null;
                if (id !== null) element = document.getElementById(id);
                else if (selector !== null) element = document.querySelector(selector);
                else if (coordinates !== null) element = document.elementFromPoint(coordinates[0], coordinates[1]);
                if (!element) return {{ok:false,error:'target element not found'}};
                try {{ element.scrollIntoView({{block:'center',inline:'center'}}); }} catch (_) {{}}
                const rect = element.getBoundingClientRect();
                const attributes = [];
                for (const attribute of element.attributes || []) attributes.push(attribute.name, attribute.value);
                let matchedCssRules = 0;
                for (const sheet of document.styleSheets || []) {{
                    let rules;
                    try {{ rules = sheet.cssRules || []; }} catch (_) {{ continue; }}
                    for (const rule of rules) {{
                        if (!rule.selectorText) continue;
                        try {{ if (element.matches(rule.selectorText)) matchedCssRules++; }} catch (_) {{}}
                    }}
                }}
                let highlight = document.getElementById('__code_inspect_highlight');
                if (!highlight) {{
                    highlight = document.createElement('div');
                    highlight.id = '__code_inspect_highlight';
                    Object.assign(highlight.style, {{
                        position:'fixed', pointerEvents:'none', zIndex:'2147483646',
                        border:'2px solid rgba(255,229,153,.95)',
                        background:'rgba(111,168,220,.20)', boxSizing:'border-box'
                    }});
                    document.documentElement.appendChild(highlight);
                }}
                Object.assign(highlight.style, {{
                    left:`${{rect.left}}px`, top:`${{rect.top}}px`,
                    width:`${{rect.width}}px`, height:`${{rect.height}}px`
                }});
                return {{
                    ok:true,
                    tagName:element.tagName || '',
                    attributes,
                    outerHTML:element.outerHTML || '',
                    boxModel:{{x:rect.x,y:rect.y,width:rect.width,height:rect.height}},
                    matchedCssRules
                }};
            }} catch (error) {{
                return {{ok:false,error:String(error)}};
            }}
        }})()"#
    );
    let raw = browser_manager
        .execute_javascript(&script)
        .await
        .map_err(|error| format!("Failed to inspect DOM target: {error}"))?;
    let value = unwrap_execute_javascript_value(raw)?;
    if value.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        Ok(value)
    } else {
        Err(value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("target element not found")
            .to_owned())
    }
}
