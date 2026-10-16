// Kudu Desktop - Read-only document viewers for the editor dock.
// Turns Office files into something the webview can render without shipping
// a JS document stack: .docx -> a small HTML fragment (only tags emitted here,
// every piece of document text escaped, so it is safe to inject), .pptx ->
// per-slide text, spreadsheets (xlsx/xls/ods via calamine) -> a capped cell
// grid. fs_extract_text gives "Ask Kudu about this file" a plain-text version
// of the same formats. Everything is local; nothing is sent anywhere here.
use std::collections::HashMap;
use std::io::{Read, Seek};

use base64::Engine;
use calamine::{Data, Reader as _};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use serde::Serialize;

/// File on disk - beyond this, "open it with its usual app" is the better UX.
const MAX_DOC_BYTES: u64 = 60 * 1024 * 1024;
/// One uncompressed zip entry (guards against zip bombs).
const MAX_XML_BYTES: u64 = 40 * 1024 * 1024;
/// All inline images of one .docx combined.
const MAX_IMAGE_BYTES: u64 = 15 * 1024 * 1024;
const MAX_HTML_CHARS: usize = 4 * 1024 * 1024;
const MAX_SLIDES: usize = 500;
const MAX_SHEET_ROWS: usize = 5_000;
const MAX_SHEET_COLS: usize = 200;
const MAX_SHEET_CELLS: usize = 400_000;
const MAX_TEXT_CHARS: usize = 200_000;

pub const DOCX_EXTS: [&str; 4] = ["docx", "docm", "dotx", "dotm"];
pub const PPTX_EXTS: [&str; 4] = ["pptx", "pptm", "ppsx", "potx"];
pub const SHEET_EXTS: [&str; 6] = ["xlsx", "xlsm", "xlsb", "xls", "xla", "ods"];

fn ext_of(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn check_size(path: &str) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("Could not open {}: {}", path, e))?;
    if meta.len() > MAX_DOC_BYTES {
        return Err(format!(
            "Too large to preview here ({} MB) - open it with its usual app instead.",
            meta.len() / 1024 / 1024
        ));
    }
    Ok(())
}

/// The parsing is CPU-bound; keep it off the main thread so a big file
/// can't freeze the window.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

// ── Zip / XML helpers ──────────────────────────────────────────────

fn open_zip(path: &str) -> Result<zip::ZipArchive<std::io::BufReader<std::fs::File>>, String> {
    check_size(path)?;
    let f = std::fs::File::open(path).map_err(|e| format!("Could not open {}: {}", path, e))?;
    zip::ZipArchive::new(std::io::BufReader::new(f)).map_err(|_| {
        "This isn't a readable Office document - it may be damaged, password-protected or in the older binary format.".to_string()
    })
}

fn read_entry<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str, limit: u64) -> Option<Vec<u8>> {
    let file = zip.by_name(name).ok()?;
    let mut buf = Vec::new();
    file.take(limit + 1).read_to_end(&mut buf).ok()?;
    (buf.len() as u64 <= limit).then_some(buf)
}

fn read_xml<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Option<String> {
    read_entry(zip, name, MAX_XML_BYTES).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Resolves a relationship target ("media/image1.png", "../slides/x.xml",
/// "/word/media/a.png") against the folder of the part that owns it.
fn resolve_target(base_dir: &str, target: &str) -> String {
    let joined = match target.strip_prefix('/') {
        Some(abs) => abs.to_string(),
        None => format!("{}/{}", base_dir, target),
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

struct Rel {
    target: String,
    kind: String,
    external: bool,
}

fn parse_rels(xml: &str) -> HashMap<String, Rel> {
    let mut out = HashMap::new();
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if e.local_name().as_ref() == b"Relationship" => {
                if let (Some(id), Some(target)) = (attr(&e, b"Id"), attr(&e, b"Target")) {
                    let external = attr(&e, b"TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("external"));
                    out.insert(id, Rel { target, kind: attr(&e, b"Type").unwrap_or_default(), external });
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

fn attr(e: &BytesStart, key: &[u8]) -> Option<String> {
    let a = e.try_get_attribute(key).ok().flatten()?;
    let raw = String::from_utf8_lossy(&a.value).into_owned();
    Some(quick_xml::escape::unescape(&raw).map(|c| c.into_owned()).unwrap_or(raw))
}

/// Text carried by an event - plain text, or an entity/char reference (which
/// quick-xml reports separately from the surrounding text).
fn event_text(ev: &Event) -> Option<String> {
    match ev {
        Event::Text(t) => t.decode().ok().map(|c| c.into_owned()),
        Event::GeneralRef(r) => match r.resolve_char_ref() {
            Ok(Some(c)) => Some(c.to_string()),
            _ => r
                .decode()
                .ok()
                .and_then(|n| quick_xml::escape::resolve_predefined_entity(&n).map(str::to_string)),
        },
        _ => None,
    }
}

/// OOXML on/off properties: <w:b/> is on, <w:b w:val="0"/> is off.
fn toggle_on(e: &BytesStart) -> bool {
    !matches!(attr(e, b"w:val").as_deref(), Some("0" | "false" | "off" | "none"))
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

// ── .docx ──────────────────────────────────────────────────────────

/// numId -> abstractNumId, and (abstractNumId, level) -> numbered vs bullet.
#[derive(Default)]
struct Numbering {
    num_to_abs: HashMap<String, String>,
    ordered: HashMap<(String, u32), bool>,
}

impl Numbering {
    fn parse(xml: &str) -> Self {
        let mut n = Numbering::default();
        let mut reader = Reader::from_str(xml);
        let (mut cur_abs, mut cur_lvl, mut cur_num) = (None::<String>, None::<u32>, None::<String>);
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.name().as_ref() {
                    b"w:abstractNum" => cur_abs = attr(&e, b"w:abstractNumId"),
                    b"w:lvl" => cur_lvl = attr(&e, b"w:ilvl").and_then(|v| v.parse().ok()),
                    b"w:numFmt" => {
                        if let (Some(a), Some(l)) = (&cur_abs, cur_lvl) {
                            let fmt = attr(&e, b"w:val").unwrap_or_default();
                            n.ordered.insert((a.clone(), l), fmt != "bullet" && fmt != "none");
                        }
                    }
                    b"w:num" => cur_num = attr(&e, b"w:numId"),
                    b"w:abstractNumId" => {
                        if let (Some(num), Some(abs)) = (&cur_num, attr(&e, b"w:val")) {
                            n.num_to_abs.insert(num.clone(), abs);
                        }
                    }
                    _ => {}
                },
                Ok(Event::End(e)) => match e.name().as_ref() {
                    b"w:abstractNum" => cur_abs = None,
                    b"w:lvl" => cur_lvl = None,
                    b"w:num" => cur_num = None,
                    _ => {}
                },
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
        n
    }

    fn is_ordered(&self, num_id: &str, level: u32) -> bool {
        self.num_to_abs
            .get(num_id)
            .and_then(|abs| self.ordered.get(&(abs.clone(), level)))
            .copied()
            .unwrap_or(false)
    }
}

#[derive(Default)]
struct Para {
    tag: &'static str,
    /// (numbered, nesting level) for list items.
    list: Option<(bool, u32)>,
    align: Option<&'static str>,
    html: String,
    text: String,
}

struct Cell {
    span: u32,
    blocks: Vec<Block>,
}

enum Block {
    Para(Para),
    Table(Vec<Vec<Cell>>),
}

#[derive(Default)]
struct Run {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    sup: bool,
    sub: bool,
    html: String,
    text: String,
}

impl Run {
    fn render(&self) -> String {
        if self.html.is_empty() {
            return String::new();
        }
        let mut s = self.html.clone();
        let wrap = |s: String, on: bool, tag: &str| if on { format!("<{tag}>{s}</{tag}>") } else { s };
        s = wrap(s, self.sup, "sup");
        s = wrap(s, self.sub && !self.sup, "sub");
        s = wrap(s, self.strike, "s");
        s = wrap(s, self.underline, "u");
        s = wrap(s, self.italic, "em");
        wrap(s, self.bold, "strong")
    }
}

/// Paragraph style id -> element. Style ids are usually the English names
/// ("Heading2"), but localized Word builds write their own ("berschrift2").
fn style_tag(style: &str) -> &'static str {
    let s = style.to_ascii_lowercase();
    if s == "title" {
        return "h1";
    }
    if s == "subtitle" {
        return "h3";
    }
    if s.contains("quote") {
        return "blockquote";
    }
    let heading = ["heading", "berschrift", "titre", "titulo", "kop"].iter().any(|h| s.contains(h));
    if heading {
        let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
        return match digits.parse::<u32>().unwrap_or(1) {
            0 | 1 => "h1",
            2 => "h2",
            3 => "h3",
            4 => "h4",
            5 => "h5",
            _ => "h6",
        };
    }
    "p"
}

fn image_mime(path: &str) -> Option<&'static str> {
    Some(match ext_of(path).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => return None, // emf/wmf/tiff: the webview can't draw them
    })
}

/// Walks word/document.xml into blocks. `image` maps a relationship id to a
/// data URL (or None when it can't be shown).
fn parse_docx_body(xml: &str, numbering: &Numbering, image: &mut dyn FnMut(&str) -> Option<String>) -> Vec<Block> {
    let mut reader = Reader::from_str(xml);
    let mut containers: Vec<Vec<Block>> = vec![Vec::new()];
    let mut tables: Vec<Vec<Vec<Cell>>> = Vec::new();
    let mut spans: Vec<u32> = Vec::new();
    let mut para: Option<Para> = None;
    let mut num: (Option<String>, u32) = (None, 0);
    let mut run: Option<Run> = None;
    let mut in_ppr = false;
    let mut in_t = false;

    loop {
        let ev = match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(ev) => ev,
        };
        match &ev {
            Event::Start(e) | Event::Empty(e) => {
                let empty = matches!(ev, Event::Empty(_));
                match e.name().as_ref() {
                    b"w:p" if !empty => {
                        para = Some(Para { tag: "p", ..Default::default() });
                        num = (None, 0);
                    }
                    b"w:pPr" if !empty => in_ppr = true,
                    b"w:pStyle" if in_ppr => {
                        if let (Some(p), Some(v)) = (para.as_mut(), attr(e, b"w:val")) {
                            p.tag = style_tag(&v);
                        }
                    }
                    b"w:numId" if in_ppr => num.0 = attr(e, b"w:val"),
                    b"w:ilvl" if in_ppr => num.1 = attr(e, b"w:val").and_then(|v| v.parse().ok()).unwrap_or(0).min(8),
                    b"w:jc" if in_ppr => {
                        if let Some(p) = para.as_mut() {
                            p.align = match attr(e, b"w:val").as_deref() {
                                Some("center") => Some("center"),
                                Some("right" | "end") => Some("right"),
                                Some("both" | "distribute") => Some("justify"),
                                _ => None,
                            };
                        }
                    }
                    b"w:r" if !empty => run = Some(Run::default()),
                    b"w:b" => {
                        if let Some(r) = run.as_mut() {
                            r.bold = toggle_on(e);
                        }
                    }
                    b"w:i" => {
                        if let Some(r) = run.as_mut() {
                            r.italic = toggle_on(e);
                        }
                    }
                    b"w:u" => {
                        if let Some(r) = run.as_mut() {
                            r.underline = toggle_on(e);
                        }
                    }
                    b"w:strike" | b"w:dstrike" => {
                        if let Some(r) = run.as_mut() {
                            r.strike = toggle_on(e);
                        }
                    }
                    b"w:vertAlign" => {
                        if let Some(r) = run.as_mut() {
                            let v = attr(e, b"w:val").unwrap_or_default();
                            r.sup = v == "superscript";
                            r.sub = v == "subscript";
                        }
                    }
                    b"w:t" if !empty => in_t = true,
                    b"w:tab" => {
                        if let Some(r) = run.as_mut() {
                            r.html.push_str("&emsp;");
                            r.text.push('\t');
                        }
                    }
                    b"w:br" | b"w:cr" => {
                        if let Some(r) = run.as_mut() {
                            if attr(e, b"w:type").as_deref() != Some("page") {
                                r.html.push_str("<br>");
                                r.text.push('\n');
                            }
                        }
                    }
                    b"w:hyperlink" if !empty => {
                        if let Some(p) = para.as_mut() {
                            p.html.push_str("<span class=\"docx-link\">");
                        }
                    }
                    b"w:drawing" if !empty => {
                        // Only the picture reference matters; text boxes and
                        // shape geometry inside a drawing are skipped.
                        let mut embed = None;
                        let mut depth = 1;
                        while depth > 0 {
                            match reader.read_event() {
                                Ok(Event::Start(d)) => {
                                    depth += 1;
                                    if d.local_name().as_ref() == b"blip" {
                                        embed = embed.or_else(|| attr(&d, b"r:embed"));
                                    }
                                }
                                Ok(Event::Empty(d)) => {
                                    if d.local_name().as_ref() == b"blip" {
                                        embed = embed.or_else(|| attr(&d, b"r:embed"));
                                    }
                                }
                                Ok(Event::End(_)) => depth -= 1,
                                Ok(Event::Eof) | Err(_) => break,
                                _ => {}
                            }
                        }
                        if let Some(p) = para.as_mut() {
                            match embed.as_deref().and_then(&mut *image) {
                                Some(src) => p.html.push_str(&format!("<img src=\"{}\" alt=\"\">", esc(&src))),
                                None => p.html.push_str("<span class=\"docx-missing\">image</span>"),
                            }
                        }
                    }
                    // Deleted revisions, field codes, legacy VML fallbacks and
                    // section/footnote plumbing carry no readable body text.
                    b"w:del" | b"w:delText" | b"w:instrText" | b"mc:Fallback" | b"w:pict" | b"w:sectPr" | b"w:rPrChange"
                    | b"w:pPrChange"
                        if !empty =>
                    {
                        let _ = reader.read_to_end(e.name());
                    }
                    b"w:tbl" if !empty => tables.push(Vec::new()),
                    b"w:tr" if !empty => {
                        if let Some(t) = tables.last_mut() {
                            t.push(Vec::new());
                        }
                    }
                    b"w:tc" if !empty => {
                        containers.push(Vec::new());
                        spans.push(1);
                    }
                    b"w:gridSpan" => {
                        if let (Some(s), Some(v)) = (spans.last_mut(), attr(e, b"w:val").and_then(|v| v.parse().ok())) {
                            *s = v;
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) => match e.name().as_ref() {
                b"w:t" => in_t = false,
                b"w:pPr" => in_ppr = false,
                b"w:r" => {
                    if let (Some(r), Some(p)) = (run.take(), para.as_mut()) {
                        p.html.push_str(&r.render());
                        p.text.push_str(&r.text);
                    }
                }
                b"w:hyperlink" => {
                    if let Some(p) = para.as_mut() {
                        p.html.push_str("</span>");
                    }
                }
                b"w:p" => {
                    if let Some(mut p) = para.take() {
                        if let Some(id) = num.0.take().filter(|id| id != "0") {
                            p.list = Some((numbering.is_ordered(&id, num.1), num.1));
                        }
                        if let Some(c) = containers.last_mut() {
                            c.push(Block::Para(p));
                        }
                    }
                }
                b"w:tc" if containers.len() > 1 => {
                    let blocks = containers.pop().unwrap_or_default();
                    let span = spans.pop().unwrap_or(1).clamp(1, 64);
                    if let Some(row) = tables.last_mut().and_then(|t| t.last_mut()) {
                        row.push(Cell { span, blocks });
                    }
                }
                b"w:tbl" => {
                    if let (Some(t), Some(c)) = (tables.pop(), containers.last_mut()) {
                        c.push(Block::Table(t));
                    }
                }
                _ => {}
            },
            other => {
                if in_t {
                    if let (Some(r), Some(t)) = (run.as_mut(), event_text(other)) {
                        r.html.push_str(&esc(&t));
                        r.text.push_str(&t);
                    }
                }
            }
        }
    }
    // Malformed nesting: keep whatever was collected rather than nothing.
    let mut out = Vec::new();
    for c in containers {
        out.extend(c);
    }
    out
}

/// Renders blocks; returns true if it stopped early at `limit` characters.
fn render_blocks(blocks: &[Block], out: &mut String, limit: Option<usize>) -> bool {
    let mut lists: Vec<bool> = Vec::new();
    let close = |lists: &mut Vec<bool>, out: &mut String| {
        if let Some(ordered) = lists.pop() {
            out.push_str(if ordered { "</ol>" } else { "</ul>" });
        }
    };
    for b in blocks {
        match b {
            Block::Para(p) if p.list.is_some() => {
                let (ordered, level) = p.list.unwrap_or((false, 0));
                let depth = level as usize + 1;
                while lists.len() > depth {
                    close(&mut lists, out);
                }
                if lists.len() == depth && lists.last() != Some(&ordered) {
                    close(&mut lists, out);
                }
                while lists.len() < depth {
                    out.push_str(if ordered { "<ol>" } else { "<ul>" });
                    lists.push(ordered);
                }
                out.push_str("<li>");
                out.push_str(&p.html);
                out.push_str("</li>");
            }
            _ => {
                while !lists.is_empty() {
                    close(&mut lists, out);
                }
                match b {
                    Block::Para(p) => {
                        if p.html.trim().is_empty() {
                            continue;
                        }
                        match p.align {
                            Some(a) => out.push_str(&format!("<{} style=\"text-align:{}\">", p.tag, a)),
                            None => out.push_str(&format!("<{}>", p.tag)),
                        }
                        out.push_str(&p.html);
                        out.push_str(&format!("</{}>", p.tag));
                    }
                    Block::Table(rows) => {
                        out.push_str("<div class=\"docx-table\"><table><tbody>");
                        for row in rows {
                            out.push_str("<tr>");
                            for cell in row {
                                if cell.span > 1 {
                                    out.push_str(&format!("<td colspan=\"{}\">", cell.span));
                                } else {
                                    out.push_str("<td>");
                                }
                                render_blocks(&cell.blocks, out, None);
                                out.push_str("</td>");
                            }
                            out.push_str("</tr>");
                        }
                        out.push_str("</tbody></table></div>");
                    }
                }
            }
        }
        if limit.is_some_and(|l| out.len() > l) {
            while !lists.is_empty() {
                close(&mut lists, out);
            }
            return true;
        }
    }
    while !lists.is_empty() {
        close(&mut lists, out);
    }
    false
}

fn blocks_to_text(blocks: &[Block], out: &mut String) {
    for b in blocks {
        match b {
            Block::Para(p) => {
                let t = p.text.trim_end();
                if t.trim().is_empty() {
                    continue;
                }
                if let Some((ordered, level)) = p.list {
                    out.push_str(&"  ".repeat(level as usize));
                    out.push_str(if ordered { "1. " } else { "- " });
                } else if let Some(n) = p.tag.strip_prefix('h').and_then(|n| n.parse::<usize>().ok()) {
                    out.push_str(&"#".repeat(n));
                    out.push(' ');
                }
                out.push_str(t);
                out.push('\n');
            }
            Block::Table(rows) => {
                for row in rows {
                    let cells: Vec<String> = row
                        .iter()
                        .map(|c| {
                            let mut s = String::new();
                            blocks_to_text(&c.blocks, &mut s);
                            s.trim().replace('\n', " ")
                        })
                        .collect();
                    out.push_str("| ");
                    out.push_str(&cells.join(" | "));
                    out.push_str(" |\n");
                }
            }
        }
    }
}

fn docx_blocks<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, with_images: bool) -> Result<Vec<Block>, String> {
    let body = read_xml(zip, "word/document.xml").ok_or("This doesn't look like a Word document (word/document.xml is missing).")?;
    let numbering = read_xml(zip, "word/numbering.xml").map(|x| Numbering::parse(&x)).unwrap_or_default();
    let rels = read_xml(zip, "word/_rels/document.xml.rels").map(|x| parse_rels(&x)).unwrap_or_default();
    let mut budget = MAX_IMAGE_BYTES;
    let mut image = |rid: &str| -> Option<String> {
        if !with_images {
            return None;
        }
        let rel = rels.get(rid).filter(|r| !r.external)?;
        let entry = resolve_target("word", &rel.target);
        let mime = image_mime(&entry)?;
        let bytes = read_entry(zip, &entry, budget)?;
        budget -= bytes.len() as u64;
        Some(format!("data:{};base64,{}", mime, base64::engine::general_purpose::STANDARD.encode(bytes)))
    };
    Ok(parse_docx_body(&body, &numbering, &mut image))
}

#[derive(Serialize)]
pub struct DocxDoc {
    pub html: String,
    pub words: usize,
    pub truncated: bool,
}

fn docx_to_html<R: Read + Seek>(zip: &mut zip::ZipArchive<R>) -> Result<DocxDoc, String> {
    let blocks = docx_blocks(zip, true)?;
    let mut text = String::new();
    blocks_to_text(&blocks, &mut text);
    let mut html = String::new();
    let truncated = render_blocks(&blocks, &mut html, Some(MAX_HTML_CHARS));
    Ok(DocxDoc { words: text.split_whitespace().count(), html, truncated })
}

#[tauri::command]
pub async fn doc_read_docx(path: String) -> Result<DocxDoc, String> {
    blocking(move || docx_to_html(&mut open_zip(&path)?)).await
}

// ── .pptx ──────────────────────────────────────────────────────────

#[derive(Serialize, Debug)]
pub struct SlideLine {
    pub text: String,
    pub level: u32,
}

#[derive(Serialize, Debug)]
pub struct Slide {
    pub number: usize,
    pub title: Option<String>,
    pub lines: Vec<SlideLine>,
    pub notes: Option<String>,
    pub images: u32,
}

/// Title, body lines and picture count of one slide (or notes page) XML.
fn parse_slide(xml: &str) -> (Option<String>, Vec<SlideLine>, u32) {
    let mut reader = Reader::from_str(xml);
    let mut title: Option<String> = None;
    let mut lines = Vec::new();
    let mut images = 0;
    // Placeholder type of the shape being read, and its paragraphs so far.
    let mut shape: Option<(Option<String>, Vec<SlideLine>)> = None;
    let mut para: Option<SlideLine> = None;
    let mut in_t = false;
    loop {
        let ev = match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(ev) => ev,
        };
        match &ev {
            Event::Start(e) | Event::Empty(e) => {
                let empty = matches!(ev, Event::Empty(_));
                match e.name().as_ref() {
                    b"p:sp" if !empty => shape = Some((None, Vec::new())),
                    b"p:ph" => {
                        if let Some(s) = shape.as_mut() {
                            s.0 = Some(attr(e, b"type").unwrap_or_else(|| "body".into()));
                        }
                    }
                    b"p:pic" => images += 1,
                    b"a:p" if !empty => para = Some(SlideLine { text: String::new(), level: 0 }),
                    b"a:pPr" => {
                        if let Some(p) = para.as_mut() {
                            p.level = attr(e, b"lvl").and_then(|v| v.parse().ok()).unwrap_or(0).min(8);
                        }
                    }
                    b"a:t" if !empty => in_t = true,
                    b"a:br" => {
                        if let Some(p) = para.as_mut() {
                            p.text.push('\n');
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) => match e.name().as_ref() {
                b"a:t" => in_t = false,
                b"a:p" => {
                    if let Some(p) = para.take().filter(|p| !p.text.trim().is_empty()) {
                        match shape.as_mut() {
                            Some(s) => s.1.push(p),
                            None => lines.push(p), // table cells etc. live outside p:sp
                        }
                    }
                }
                b"p:sp" => {
                    if let Some((ph, paras)) = shape.take() {
                        match ph.as_deref() {
                            Some("title" | "ctrTitle") if title.is_none() => {
                                let t: Vec<&str> = paras.iter().map(|p| p.text.trim()).collect();
                                title = Some(t.join(" ").replace('\n', " "));
                            }
                            Some("sldNum" | "dt" | "ftr" | "hdr" | "sldImg") => {}
                            _ => lines.extend(paras),
                        }
                    }
                }
                _ => {}
            },
            other => {
                if in_t {
                    if let (Some(p), Some(t)) = (para.as_mut(), event_text(other)) {
                        p.text.push_str(&t);
                    }
                }
            }
        }
    }
    (title, lines, images)
}

fn pptx_slides<R: Read + Seek>(zip: &mut zip::ZipArchive<R>) -> Result<Vec<Slide>, String> {
    // Slide order comes from presentation.xml (files can be renumbered by
    // reordering); fall back to the slideN.xml numbering.
    let pres_rels = read_xml(zip, "ppt/_rels/presentation.xml.rels").map(|x| parse_rels(&x)).unwrap_or_default();
    let mut paths: Vec<String> = Vec::new();
    if let Some(pres) = read_xml(zip, "ppt/presentation.xml") {
        let mut reader = Reader::from_str(&pres);
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) if e.name().as_ref() == b"p:sldId" => {
                    if let Some(rel) = attr(&e, b"r:id").and_then(|id| pres_rels.get(&id)) {
                        paths.push(resolve_target("ppt", &rel.target));
                    }
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
    }
    if paths.is_empty() {
        let mut numbered: Vec<(u32, String)> = zip
            .file_names()
            .filter_map(|n| {
                let num = n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse().ok()?;
                Some((num, n.to_string()))
            })
            .collect();
        numbered.sort();
        paths = numbered.into_iter().map(|(_, n)| n).collect();
    }
    if paths.is_empty() {
        return Err("This doesn't look like a PowerPoint presentation (no slides found).".into());
    }

    let mut slides = Vec::new();
    for (i, path) in paths.iter().take(MAX_SLIDES).enumerate() {
        let Some(xml) = read_xml(zip, path) else { continue };
        let (title, lines, images) = parse_slide(&xml);
        // Speaker notes hang off the slide's own relationships.
        let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
        let notes = read_xml(zip, &format!("{}/_rels/{}.rels", dir, file))
            .map(|x| parse_rels(&x))
            .and_then(|rels| rels.into_values().find(|r| r.kind.ends_with("/notesSlide")))
            .and_then(|r| read_xml(zip, &resolve_target(dir, &r.target)))
            .map(|x| parse_slide(&x).1.into_iter().map(|l| l.text).collect::<Vec<_>>().join("\n"))
            .filter(|n| !n.trim().is_empty());
        slides.push(Slide { number: i + 1, title, lines, notes, images });
    }
    Ok(slides)
}

#[tauri::command]
pub async fn doc_read_pptx(path: String) -> Result<Vec<Slide>, String> {
    blocking(move || pptx_slides(&mut open_zip(&path)?)).await
}

// ── Spreadsheets (xlsx / xls / ods) ────────────────────────────────

#[derive(Serialize)]
pub struct SheetData {
    pub sheets: Vec<String>,
    pub sheet: String,
    pub rows: Vec<Vec<String>>,
    /// 1-based spreadsheet coordinates of rows[0][0] (the used range rarely
    /// starts at A1), so the grid can label rows and columns like Excel.
    pub first_row: u32,
    pub first_col: u32,
    pub total_rows: usize,
    pub total_cols: usize,
    pub truncated: bool,
}

fn cell_text(d: &Data) -> String {
    match d {
        Data::Empty => String::new(),
        Data::Float(f) => {
            if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", *f as i64)
            } else {
                // Trim binary noise (0.1 + 0.2) without hiding real precision.
                format!("{}", (f * 1e10).round() / 1e10)
            }
        }
        Data::DateTime(dt) => match dt.as_datetime() {
            Some(v) if dt.is_datetime() => {
                if v.time() == chrono::NaiveTime::MIN {
                    v.format("%Y-%m-%d").to_string()
                } else if v.date() == chrono::NaiveDate::from_ymd_opt(1899, 12, 30).unwrap_or_default() {
                    v.format("%H:%M:%S").to_string()
                } else {
                    v.format("%Y-%m-%d %H:%M").to_string()
                }
            }
            _ => dt.to_string(),
        },
        other => other.to_string(),
    }
}

fn read_sheet<RS: Read + Seek>(wb: &mut calamine::Sheets<RS>, sheet: Option<String>) -> Result<SheetData, String> {
    let sheets = wb.sheet_names();
    let name = sheet
        .filter(|s| sheets.contains(s))
        .or_else(|| sheets.first().cloned())
        .ok_or("This workbook has no sheets.")?;
    let range = wb.worksheet_range(&name).map_err(|e| format!("Could not read sheet \"{}\": {}", name, e))?;
    let (height, width) = range.get_size();
    let (r0, c0) = range.start().unwrap_or((0, 0));
    let cols = width.min(MAX_SHEET_COLS);
    let max_rows = MAX_SHEET_ROWS.min(MAX_SHEET_CELLS / cols.max(1));
    let rows: Vec<Vec<String>> = range
        .rows()
        .take(max_rows)
        .map(|r| {
            let mut cells: Vec<String> = r.iter().take(cols).map(cell_text).collect();
            while cells.last().is_some_and(|c| c.is_empty()) {
                cells.pop();
            }
            cells
        })
        .collect();
    Ok(SheetData {
        truncated: height > rows.len() || width > cols,
        sheets,
        sheet: name,
        rows,
        first_row: r0 + 1,
        first_col: c0 + 1,
        total_rows: height,
        total_cols: width,
    })
}

#[tauri::command]
pub async fn doc_read_sheet(path: String, sheet: Option<String>) -> Result<SheetData, String> {
    blocking(move || {
        check_size(&path)?;
        let mut wb = calamine::open_workbook_auto(&path).map_err(|e| format!("Could not open this spreadsheet: {}", e))?;
        read_sheet(&mut wb, sheet)
    })
    .await
}

// ── Plain text for the chat ("Ask Kudu about this file") ─────────────

fn cap_text(mut s: String) -> String {
    if s.len() > MAX_TEXT_CHARS {
        let mut cut = MAX_TEXT_CHARS;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n… (truncated)");
    }
    s
}

pub(crate) fn extract_text(path: &str) -> Result<String, String> {
    let ext = ext_of(path);
    if DOCX_EXTS.contains(&ext.as_str()) {
        let blocks = docx_blocks(&mut open_zip(path)?, false)?;
        let mut out = String::new();
        blocks_to_text(&blocks, &mut out);
        return Ok(cap_text(out));
    }
    if PPTX_EXTS.contains(&ext.as_str()) {
        let mut out = String::new();
        for s in pptx_slides(&mut open_zip(path)?)? {
            out.push_str(&format!("## Slide {}{}\n", s.number, s.title.map(|t| format!(": {}", t)).unwrap_or_default()));
            for l in s.lines {
                out.push_str(&format!("{}- {}\n", "  ".repeat(l.level as usize), l.text.replace('\n', " ")));
            }
            if let Some(n) = s.notes {
                out.push_str(&format!("Notes: {}\n", n));
            }
            out.push('\n');
        }
        return Ok(cap_text(out));
    }
    if SHEET_EXTS.contains(&ext.as_str()) {
        check_size(path)?;
        let mut wb = calamine::open_workbook_auto(path).map_err(|e| format!("Could not open this spreadsheet: {}", e))?;
        let mut out = String::new();
        for name in wb.sheet_names() {
            let data = read_sheet(&mut wb, Some(name.clone()))?;
            out.push_str(&format!("## Sheet: {} ({} rows x {} columns)\n", name, data.total_rows, data.total_cols));
            for row in data.rows {
                out.push_str(&row.join("\t"));
                out.push('\n');
                if out.len() > MAX_TEXT_CHARS {
                    return Ok(cap_text(out));
                }
            }
            out.push('\n');
        }
        return Ok(cap_text(out));
    }
    // Anything else: read it as text if it is text.
    let f = std::fs::File::open(path).map_err(|e| format!("Could not open {}: {}", path, e))?;
    let mut bytes = Vec::new();
    f.take(MAX_TEXT_CHARS as u64 * 2)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Could not read {}: {}", path, e))?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Err("It's a binary file, so there's no text to share.".into());
    }
    Ok(cap_text(String::from_utf8_lossy(&bytes).into_owned()))
}

#[tauri::command]
pub async fn fs_extract_text(path: String) -> Result<String, String> {
    blocking(move || extract_text(&path)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn build_zip(files: &[(&str, &str)]) -> zip::ZipArchive<Cursor<Vec<u8>>> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in files {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        let cursor = w.finish().unwrap();
        zip::ZipArchive::new(Cursor::new(cursor.into_inner())).unwrap()
    }

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    #[test]
    fn docx_headings_runs_lists_and_tables() {
        let doc = format!(
            r#"<?xml version="1.0"?><w:document {W}><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Report &amp; plan</w:t></w:r></w:p>
<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Bold</w:t></w:r><w:r><w:t xml:space="preserve"> and </w:t></w:r><w:r><w:rPr><w:i/></w:rPr><w:t>&lt;italic&gt;</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>first</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="2"/></w:numPr></w:pPr><w:r><w:t>step</w:t></w:r></w:p>
<w:tbl><w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>x</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>y</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:p><w:r><w:delText>gone</w:delText></w:r></w:p>
</w:body></w:document>"#
        );
        let numbering = format!(
            r#"<w:numbering {W}><w:abstractNum w:abstractNumId="10"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum>
<w:abstractNum w:abstractNumId="11"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum>
<w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="11"/></w:num></w:numbering>"#
        );
        let mut zip = build_zip(&[("word/document.xml", &doc), ("word/numbering.xml", &numbering)]);
        let out = docx_to_html(&mut zip).unwrap();
        assert!(out.html.contains("<h1>Report &amp; plan</h1>"), "{}", out.html);
        assert!(out.html.contains("<strong>Bold</strong> and <em>&lt;italic&gt;</em>"), "{}", out.html);
        assert!(out.html.contains("<ul><li>first</li></ul><ol><li>step</li></ol>"), "{}", out.html);
        assert!(out.html.contains("<td colspan=\"2\"><p>A1</p></td>"), "{}", out.html);
        assert!(!out.html.contains("gone"));
        assert!(!out.truncated);
        assert!(out.words >= 9);
    }

    #[test]
    fn docx_missing_body_is_an_error() {
        let mut zip = build_zip(&[("other.xml", "<x/>")]);
        assert!(docx_to_html(&mut zip).is_err());
    }

    #[test]
    fn pptx_slides_in_presentation_order_with_notes() {
        let slide = |title: &str, body: &str| {
            format!(
                r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree>
<p:sp><p:nvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>{title}</a:t></a:r></a:p></p:txBody></p:sp>
<p:sp><p:nvSpPr><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>{body}</a:t></a:r></a:p><a:p><a:pPr lvl="1"/><a:r><a:t>sub</a:t></a:r></a:p></p:txBody></p:sp>
<p:sp><p:nvSpPr><p:nvPr><p:ph type="sldNum"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>7</a:t></a:r></a:p></p:txBody></p:sp>
<p:pic/></p:spTree></p:cSld></p:sld>"#
            )
        };
        let (s1, s2) = (slide("Intro", "Hello"), slide("End", "Bye"));
        let pres = r#"<p:presentation xmlns:p="p" xmlns:r="r"><p:sldIdLst><p:sldId id="1" r:id="rId2"/><p:sldId id="2" r:id="rId1"/></p:sldIdLst></p:presentation>"#;
        let pres_rels = r#"<Relationships><Relationship Id="rId1" Type="t/slide" Target="slides/slide1.xml"/><Relationship Id="rId2" Type="t/slide" Target="slides/slide2.xml"/></Relationships>"#;
        let slide_rels = r#"<Relationships><Relationship Id="rId9" Type="http://x/notesSlide" Target="../notesSlides/notesSlide1.xml"/></Relationships>"#;
        let notes = r#"<p:notes xmlns:p="p" xmlns:a="a"><p:sp><p:nvSpPr><p:nvPr><p:ph type="body"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>Say hi</a:t></a:r></a:p></p:txBody></p:sp></p:notes>"#;
        let mut zip = build_zip(&[
            ("ppt/presentation.xml", pres),
            ("ppt/_rels/presentation.xml.rels", pres_rels),
            ("ppt/slides/slide1.xml", &s1),
            ("ppt/slides/slide2.xml", &s2),
            ("ppt/slides/_rels/slide2.xml.rels", slide_rels),
            ("ppt/notesSlides/notesSlide1.xml", notes),
        ]);
        let slides = pptx_slides(&mut zip).unwrap();
        assert_eq!(slides.len(), 2);
        assert_eq!(slides[0].title.as_deref(), Some("End"));
        assert_eq!(slides[0].notes.as_deref(), Some("Say hi"));
        assert_eq!(slides[1].title.as_deref(), Some("Intro"));
        assert_eq!(slides[1].lines.len(), 2);
        assert_eq!(slides[1].lines[1].level, 1);
        assert_eq!(slides[1].images, 1);
        assert!(slides[1].notes.is_none());
    }

    #[test]
    fn xlsx_grid_with_offsets_and_types() {
        let files = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>
<row r="2"><c r="B2" t="inlineStr"><is><t>Name</t></is></c><c r="C2" t="inlineStr"><is><t>Score</t></is></c></row>
<row r="3"><c r="B3" t="inlineStr"><is><t>Ada</t></is></c><c r="C3"><v>42</v></c></row>
<row r="4"><c r="B4" t="inlineStr"><is><t>Bob</t></is></c><c r="C4"><v>0.30000000000000004</v></c></row>
</sheetData></worksheet>"#,
            ),
        ];
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in files {
            w.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        let bytes = w.finish().unwrap().into_inner();
        let mut wb = calamine::open_workbook_auto_from_rs(Cursor::new(bytes)).unwrap();
        let data = read_sheet(&mut wb, None).unwrap();
        assert_eq!(data.sheets, vec!["Data".to_string()]);
        assert_eq!((data.first_row, data.first_col), (2, 2));
        assert_eq!(data.rows[0], vec!["Name", "Score"]);
        assert_eq!(data.rows[1], vec!["Ada", "42"]);
        assert_eq!(data.rows[2], vec!["Bob", "0.3"]);
        assert!(!data.truncated);
    }

    #[test]
    fn targets_resolve_relative_and_absolute() {
        assert_eq!(resolve_target("ppt/slides", "../notesSlides/n1.xml"), "ppt/notesSlides/n1.xml");
        assert_eq!(resolve_target("word", "/word/media/a.png"), "word/media/a.png");
        assert_eq!(resolve_target("word", "media/a.png"), "word/media/a.png");
    }
}
