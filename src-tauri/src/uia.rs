// Mouse AI Desktop - UI Automation (element-level targeting for the agent).
//
// Until now the agent could only click screenshot coordinates and type into whatever
// had focus, and could only READ the screen through a 12x10 MSAA point sample. This
// module walks the foreground window's UI Automation tree so the agent can address
// controls the way a person names them - "the Save button", "the Search box" - and act
// on them through the accessibility API (Invoke / Toggle / SetValue) or with a real
// click on the element's own rectangle. That is what makes a desktop task survive a
// window that moved, a dialog that opened a few pixels lower, or a DPI change.
//
// Everything here is blocking Win32/COM and must run on a `spawn_blocking` thread; each
// call initializes an STA apartment for itself (same pattern as mouse.rs). COM pointers
// are never cached across calls: elements are described by plain data and re-found.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct UiElement {
    /// Stable within one find_elements result: "u1", "u2", ... (re-found by name/role/rect).
    pub id: String,
    pub role: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub automation_id: String,
    /// Screen pixels (virtual desktop), like the rest of the app.
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub enabled: bool,
    pub offscreen: bool,
    /// Which UIA patterns the element supports: invoke | toggle | value | select | expand
    pub can: Vec<&'static str>,
}

/// A request to locate elements. Empty query = the interactive controls of the window.
#[derive(Debug, Clone, Default)]
pub struct Query {
    pub text: String,
    pub role: String,
    pub limit: usize,
    /// Include static text / panes (for reading), not just controls.
    pub include_text: bool,
}

#[cfg(windows)]
mod win {
    use super::{Query, UiElement};
    use windows::core::{Interface, BSTR};
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationCondition, IUIAutomationElement, IUIAutomationExpandCollapsePattern,
        IUIAutomationInvokePattern, IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern,
        IUIAutomationValuePattern, TreeScope_Descendants, UIA_CONTROLTYPE_ID, UIA_ExpandCollapsePatternId,
        UIA_InvokePatternId, UIA_SelectionItemPatternId, UIA_TogglePatternId, UIA_ValuePatternId,
    };

    /// Control type ids (UIAutomationClient.h) → the words a person uses.
    fn role_name(id: i32) -> &'static str {
        match id {
            50000 => "button",
            50001 => "calendar",
            50002 => "checkbox",
            50003 => "combobox",
            50004 => "edit",
            50005 => "link",
            50006 => "image",
            50007 => "listitem",
            50008 => "list",
            50009 => "menu",
            50010 => "menubar",
            50011 => "menuitem",
            50012 => "progressbar",
            50013 => "radio",
            50014 => "scrollbar",
            50015 => "slider",
            50016 => "spinner",
            50017 => "statusbar",
            50018 => "tab",
            50019 => "tabitem",
            50020 => "text",
            50021 => "toolbar",
            50022 => "tooltip",
            50023 => "tree",
            50024 => "treeitem",
            50025 => "custom",
            50026 => "group",
            50027 => "thumb",
            50028 => "datagrid",
            50029 => "dataitem",
            50030 => "document",
            50031 => "splitbutton",
            50032 => "window",
            50033 => "pane",
            50034 => "header",
            50035 => "headeritem",
            50036 => "table",
            50037 => "titlebar",
            50038 => "separator",
            _ => "control",
        }
    }

    fn role_id(name: &str) -> Option<i32> {
        let n = name.to_lowercase();
        let id = match n.as_str() {
            "button" => 50000,
            "checkbox" | "check" => 50002,
            "combobox" | "dropdown" | "combo" => 50003,
            "edit" | "textbox" | "input" | "field" => 50004,
            "link" | "hyperlink" => 50005,
            "image" => 50006,
            "listitem" | "item" => 50007,
            "list" => 50008,
            "menu" => 50009,
            "menuitem" => 50011,
            "radio" | "radiobutton" => 50013,
            "slider" => 50015,
            "tab" | "tabitem" => 50019,
            "text" | "label" => 50020,
            "treeitem" => 50024,
            "document" | "editor" => 50030,
            "splitbutton" => 50031,
            "window" => 50032,
            "dataitem" | "cell" => 50029,
            _ => return None,
        };
        Some(id)
    }

    /// Controls worth listing without a query. Static text and panes are noise unless
    /// the caller asked for them or they match the query.
    fn is_control(id: i32) -> bool {
        matches!(
            id,
            50000 | 50002 | 50003 | 50004 | 50005 | 50007 | 50011 | 50013 | 50015 | 50016 | 50019 | 50024 | 50029 | 50030 | 50031
        )
    }

    struct Com;
    impl Com {
        fn init() -> Com {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            Com
        }
    }
    impl Drop for Com {
        fn drop(&mut self) {
            unsafe { CoUninitialize() }
        }
    }

    fn automation() -> Result<IUIAutomation, String> {
        unsafe {
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).map_err(|e| format!("UI Automation unavailable: {}", e))
        }
    }

    fn foreground() -> Result<HWND, String> {
        let hwnd = unsafe { winapi::um::winuser::GetForegroundWindow() };
        if hwnd.is_null() {
            return Err("no foreground window".to_string());
        }
        Ok(HWND(hwnd as *mut core::ffi::c_void))
    }

    fn describe(el: &IUIAutomationElement, idx: usize) -> Option<UiElement> {
        unsafe {
            let ct: UIA_CONTROLTYPE_ID = el.CurrentControlType().ok()?;
            let rect: RECT = el.CurrentBoundingRectangle().unwrap_or_default();
            let name = el.CurrentName().map(|b| b.to_string()).unwrap_or_default();
            let automation_id = el.CurrentAutomationId().map(|b| b.to_string()).unwrap_or_default();
            let enabled = el.CurrentIsEnabled().map(|b| b.as_bool()).unwrap_or(true);
            let offscreen = el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false);
            let mut can = Vec::new();
            let mut value = String::new();
            if let Ok(p) = el.GetCurrentPattern(UIA_ValuePatternId) {
                if let Ok(vp) = p.cast::<IUIAutomationValuePattern>() {
                    can.push("value");
                    value = vp.CurrentValue().map(|b| b.to_string()).unwrap_or_default();
                }
            }
            if el.GetCurrentPattern(UIA_InvokePatternId).is_ok() {
                can.push("invoke");
            }
            if el.GetCurrentPattern(UIA_TogglePatternId).is_ok() {
                can.push("toggle");
            }
            if el.GetCurrentPattern(UIA_SelectionItemPatternId).is_ok() {
                can.push("select");
            }
            if el.GetCurrentPattern(UIA_ExpandCollapsePatternId).is_ok() {
                can.push("expand");
            }
            let clip = |s: String| -> String { if s.chars().count() > 160 { s.chars().take(160).collect::<String>() + "…" } else { s } };
            Some(UiElement {
                id: format!("u{}", idx),
                role: role_name(ct.0).to_string(),
                name: clip(name),
                value: clip(value),
                automation_id: clip(automation_id),
                x: rect.left,
                y: rect.top,
                w: (rect.right - rect.left).max(0),
                h: (rect.bottom - rect.top).max(0),
                enabled,
                offscreen,
                can,
            })
        }
    }

    fn collect(auto: &IUIAutomation, root: &IUIAutomationElement, q: &Query) -> Result<Vec<(UiElement, IUIAutomationElement)>, String> {
        unsafe {
            let cond: IUIAutomationCondition = match role_id(&q.role) {
                Some(id) => auto
                    .CreatePropertyCondition(windows::Win32::UI::Accessibility::UIA_ControlTypePropertyId, &VARIANT::from(id))
                    .map_err(|e| e.to_string())?,
                None => auto.CreateTrueCondition().map_err(|e| e.to_string())?,
            };
            let found = root.FindAll(TreeScope_Descendants, &cond).map_err(|e| format!("tree walk failed: {}", e))?;
            let n = found.Length().unwrap_or(0).max(0) as usize;
            let needle = q.text.trim().to_lowercase();
            let mut out = Vec::new();
            // Hard cap on the walk: a browser tab can expose thousands of nodes.
            for i in 0..n.min(2500) {
                let Ok(el) = found.GetElement(i as i32) else { continue };
                let Some(mut d) = describe(&el, out.len() + 1) else { continue };
                let ct = role_id(&d.role).unwrap_or(0);
                let matches_text = !needle.is_empty()
                    && (d.name.to_lowercase().contains(&needle)
                        || d.value.to_lowercase().contains(&needle)
                        || d.automation_id.to_lowercase().contains(&needle));
                if !needle.is_empty() && !matches_text {
                    continue;
                }
                if needle.is_empty() && q.role.is_empty() && !q.include_text && !is_control(ct) {
                    continue;
                }
                if d.name.is_empty() && d.value.is_empty() && d.automation_id.is_empty() && needle.is_empty() {
                    continue;
                }
                if d.w == 0 && d.h == 0 && !matches_text {
                    continue;
                }
                d.id = format!("u{}", out.len() + 1);
                out.push((d, el));
                if out.len() >= q.limit.max(1) * 3 {
                    break;
                }
            }
            Ok(out)
        }
    }

    fn rank(list: &mut Vec<(UiElement, IUIAutomationElement)>, q: &Query) {
        let needle = q.text.trim().to_lowercase();
        list.sort_by_key(|(d, _)| {
            let exact = !needle.is_empty() && d.name.to_lowercase() == needle;
            let starts = !needle.is_empty() && d.name.to_lowercase().starts_with(&needle);
            let mut score = 0i32;
            if exact { score -= 40 }
            if starts { score -= 15 }
            if d.offscreen { score += 20 }
            if !d.enabled { score += 10 }
            if d.can.iter().any(|c| *c == "invoke" || *c == "value" || *c == "toggle" || *c == "select") { score -= 5 }
            score
        });
    }

    pub fn find(q: &Query) -> Result<Vec<UiElement>, String> {
        let _com = Com::init();
        let auto = automation()?;
        let hwnd = foreground()?;
        let root = unsafe { auto.ElementFromHandle(hwnd) }.map_err(|e| format!("no accessibility tree for the foreground window: {}", e))?;
        let mut list = collect(&auto, &root, q)?;
        rank(&mut list, q);
        let limit = q.limit.max(1);
        Ok(list.into_iter().take(limit).enumerate().map(|(i, (mut d, _))| { d.id = format!("u{}", i + 1); d }).collect())
    }

    /// Re-find one element: by its listed name+role (closest to the remembered rect),
    /// or by name alone when no listing exists.
    fn locate(auto: &IUIAutomation, root: &IUIAutomationElement, name: &str, role: &str, near: Option<(i32, i32)>) -> Result<(UiElement, IUIAutomationElement), String> {
        let q = Query { text: name.to_string(), role: role.to_string(), limit: 12, include_text: true };
        let mut list = collect(auto, root, &q)?;
        if list.is_empty() {
            return Err(format!("no element named \"{}\" in the foreground window (it may have changed - call find_elements again)", name));
        }
        rank(&mut list, &q);
        if let Some((nx, ny)) = near {
            list.sort_by_key(|(d, _)| (d.x + d.w / 2 - nx).abs() + (d.y + d.h / 2 - ny).abs());
        }
        Ok(list.remove(0))
    }

    pub enum Act {
        Invoke,
        Toggle,
        Select,
        Expand,
        SetValue(String),
        Focus,
    }

    /// Act on an element through its accessibility pattern. Returns a short receipt;
    /// Err when the pattern is not supported (the caller then clicks for real).
    pub fn act(name: &str, role: &str, near: Option<(i32, i32)>, action: Act) -> Result<(UiElement, String), String> {
        let _com = Com::init();
        let auto = automation()?;
        let hwnd = foreground()?;
        let root = unsafe { auto.ElementFromHandle(hwnd) }.map_err(|e| e.to_string())?;
        let (desc, el) = locate(&auto, &root, name, role, near)?;
        unsafe {
            let receipt = match action {
                Act::Focus => {
                    el.SetFocus().map_err(|e| format!("could not focus: {}", e))?;
                    "focused".to_string()
                }
                Act::Invoke => {
                    let p = el.GetCurrentPattern(UIA_InvokePatternId).map_err(|_| "element has no Invoke pattern")?;
                    let ip = p.cast::<IUIAutomationInvokePattern>().map_err(|e| e.to_string())?;
                    ip.Invoke().map_err(|e| format!("invoke failed: {}", e))?;
                    "invoked".to_string()
                }
                Act::Toggle => {
                    let p = el.GetCurrentPattern(UIA_TogglePatternId).map_err(|_| "element has no Toggle pattern")?;
                    let tp = p.cast::<IUIAutomationTogglePattern>().map_err(|e| e.to_string())?;
                    tp.Toggle().map_err(|e| format!("toggle failed: {}", e))?;
                    "toggled".to_string()
                }
                Act::Select => {
                    let p = el.GetCurrentPattern(UIA_SelectionItemPatternId).map_err(|_| "element has no SelectionItem pattern")?;
                    let sp = p.cast::<IUIAutomationSelectionItemPattern>().map_err(|e| e.to_string())?;
                    sp.Select().map_err(|e| format!("select failed: {}", e))?;
                    "selected".to_string()
                }
                Act::Expand => {
                    let p = el.GetCurrentPattern(UIA_ExpandCollapsePatternId).map_err(|_| "element has no ExpandCollapse pattern")?;
                    let ep = p.cast::<IUIAutomationExpandCollapsePattern>().map_err(|e| e.to_string())?;
                    ep.Expand().map_err(|e| format!("expand failed: {}", e))?;
                    "expanded".to_string()
                }
                Act::SetValue(text) => {
                    let p = el.GetCurrentPattern(UIA_ValuePatternId).map_err(|_| "element has no Value pattern")?;
                    let vp = p.cast::<IUIAutomationValuePattern>().map_err(|e| e.to_string())?;
                    if vp.CurrentIsReadOnly().map(|b| b.as_bool()).unwrap_or(false) {
                        return Err("the field is read-only".to_string());
                    }
                    vp.SetValue(&BSTR::from(text.as_str())).map_err(|e| format!("set value failed: {}", e))?;
                    let now = vp.CurrentValue().map(|b| b.to_string()).unwrap_or_default();
                    format!("value set (now \"{}\")", if now.chars().count() > 80 { now.chars().take(80).collect::<String>() + "…" } else { now })
                }
            };
            Ok((desc, receipt))
        }
    }

    /// Where to click for a named element (screen pixels, centre of its rectangle).
    pub fn center_of(name: &str, role: &str, near: Option<(i32, i32)>) -> Result<UiElement, String> {
        let _com = Com::init();
        let auto = automation()?;
        let hwnd = foreground()?;
        let root = unsafe { auto.ElementFromHandle(hwnd) }.map_err(|e| e.to_string())?;
        let (desc, _) = locate(&auto, &root, name, role, near)?;
        Ok(desc)
    }
}

#[cfg(windows)]
pub use win::Act;
#[cfg(windows)]
pub fn find_elements(q: &Query) -> Result<Vec<UiElement>, String> {
    win::find(q)
}
#[cfg(windows)]
pub fn act_on(name: &str, role: &str, near: Option<(i32, i32)>, action: Act) -> Result<(UiElement, String), String> {
    win::act(name, role, near, action)
}
#[cfg(windows)]
pub fn locate(name: &str, role: &str, near: Option<(i32, i32)>) -> Result<UiElement, String> {
    win::center_of(name, role, near)
}

#[cfg(not(windows))]
pub enum Act {
    Invoke,
    Toggle,
    Select,
    Expand,
    SetValue(String),
    Focus,
}
#[cfg(not(windows))]
pub fn find_elements(_q: &Query) -> Result<Vec<UiElement>, String> {
    Err("UI Automation is only available on Windows".to_string())
}
#[cfg(not(windows))]
pub fn act_on(_n: &str, _r: &str, _near: Option<(i32, i32)>, _a: Act) -> Result<(UiElement, String), String> {
    Err("UI Automation is only available on Windows".to_string())
}
#[cfg(not(windows))]
pub fn locate(_n: &str, _r: &str, _near: Option<(i32, i32)>) -> Result<UiElement, String> {
    Err("UI Automation is only available on Windows".to_string())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Needs a desktop session: lists whatever window is in the foreground.
    #[test]
    #[ignore]
    fn lists_foreground_elements() {
        let list = find_elements(&Query { text: String::new(), role: String::new(), limit: 25, include_text: true }).unwrap();
        println!("{} element(s)", list.len());
        for e in &list {
            println!("{} {:10} {:?} value={:?} {}x{} at ({},{}) can={:?}", e.id, e.role, e.name, e.value, e.w, e.h, e.x, e.y, e.can);
        }
        assert!(!list.is_empty(), "the foreground window exposed no elements");
    }
}
