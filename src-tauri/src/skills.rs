// Kudu Desktop - Skills + real Word documents.
//
// Skills are playbooks the agent loads on demand (the same idea as Claude Code's
// skills / claw-code's Skill tool): the system prompt only lists each skill's name
// and one-line description, and `use_skill` returns the full instructions when a
// task matches - so knowing how to do twenty kinds of job costs a few lines of
// prompt, not twenty pages.
//
// Two sources:
//   • built-in skills (below), shipped with the app;
//   • the user's own: `%APPDATA%\mouse-ai\skills\<name>\SKILL.md` or `<name>.md`,
//     optional frontmatter `name:` / `description:`. A user skill with the same
//     name overrides the built-in one.
//
// `create_docx` turns Markdown into a genuine .docx (headings, lists, bold/italic,
// code, tables) with nothing but the zip crate - "make me a document" used to end
// in a .md or .txt the user then had to convert.

use serde_json::{json, Value};
use std::io::{Cursor, Write};

pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub user: bool,
}

const BUILTIN: &[(&str, &str, &str)] = &[
    (
        "word-document",
        "Write a polished document (report, docs, proposal, notes, letter) and save it as a real Word file",
        "1. Gather the content first: read the page/files/sources the task points at (read_file, web_fetch, \
delegate_to_browser for logged-in or interactive sites). Never invent facts - mark unknowns.\n\
2. Draft in Markdown: a '# Title', a 2-3 sentence summary, '## ' sections, bullets for lists, a table for comparisons, \
and a short '## Sources' list when you used the web.\n\
3. Save it with create_document to the folder the user named (default: Documents\\<Title>.docx). Use the real \
folder paths from [ENVIRONMENT].\n\
4. Verify: list_directory the folder and confirm the file is there with a non-zero size.\n\
5. Final answer: the exact absolute path, what the document covers, and anything you could not find.",
    ),
    (
        "research-report",
        "Research a topic from several sources and deliver a cited summary (optionally saved as a document)",
        "1. Break the question into 2-4 sub-questions. For each, web_search, then web_fetch the 2-3 best results \
(official sites, docs, reputable outlets). Use delegate_to_browser(background=true) for sites that need a login or \
clicking, and keep researching while it works.\n\
2. Keep notes with update_plan(notes=...) as you go: claim → source URL.\n\
3. Cross-check numbers and dates between sources; say when sources disagree.\n\
4. Answer: key findings first, then details, then '## Sources' with every URL you used. If the user wants a file, \
follow the word-document skill.",
    ),
    (
        "organize-folder",
        "Tidy a folder: sort files into subfolders by type/date/project, find duplicates, clean up safely",
        "1. list_directory the folder (and run_command `Get-ChildItem -Recurse | Group-Object Extension` for big ones) \
to see what is there.\n\
2. Propose a plan (which subfolders, what goes where) and call ask_user to confirm before moving anything.\n\
3. Move with run_command `Move-Item -LiteralPath <src> -Destination <dst>`; create folders with New-Item -ItemType \
Directory -Force. Never delete - move unwanted items to a 'To review' folder instead.\n\
4. Verify with list_directory, then report counts per folder and the folder path.",
    ),
    (
        "run-project",
        "Install, build, run or test a code project (Node, Python, .NET, Rust, Go…)",
        "1. Identify the stack from its files (package.json + lockfile, pyproject/requirements.txt, *.csproj, Cargo.toml, \
go.mod). Use the matching package manager (pnpm/yarn/npm by lockfile; uv/pip; dotnet; cargo; go).\n\
2. Always pass the project folder as cwd. Installs/builds: timeout_seconds 300. Dev servers/watchers: \
background=true, then read_file the log until it shows the URL/port or an error.\n\
3. On failure read the END of the output (the error and [exit code]), fix the cause, re-run. Don't repeat an \
unchanged command.\n\
4. Report what ran, the result (tests passed/failed counts, URL of the running app) and any remaining errors.",
    ),
    (
        "fix-error",
        "Diagnose and fix an error message, crash or failing command",
        "1. Reproduce it: run the failing command (or read the log) and capture the exact error text.\n\
2. Locate the cause: search_in_files for the symbol/message, read_file around the line, check versions/config.\n\
3. Make the smallest fix with edit_file (read_file first; exact unique snippet).\n\
4. Re-run the same command and confirm the error is gone and nothing new broke. If it still fails, go back to 2 \
with the new error - never claim it's fixed without a clean re-run.",
    ),
    (
        "git-workflow",
        "Check status, review changes, commit, branch, pull/push with git safely",
        "1. git status and git diff --stat first (cwd = the repo). Read the diff before writing a commit message.\n\
2. Commit message: imperative summary line (≤72 chars) + why. Stage only the files that belong to the change.\n\
3. Never force-push, rewrite history or delete branches unless the user explicitly asked; ask_user before any push.\n\
4. Credential prompts can't be answered here (GIT_TERMINAL_PROMPT=0): if auth fails, tell the user to sign in.",
    ),
    (
        "spreadsheet-data",
        "Build a table/spreadsheet (CSV that opens in Excel) from data on the web, in files, or from the user",
        "1. Collect the rows: web_fetch / delegate_to_browser (it can extract tables as JSON) / read_file.\n\
2. Clean them: consistent columns, numbers without units in numeric columns, ISO dates.\n\
3. Save as CSV with run_command: build objects and `Export-Csv -LiteralPath <path> -NoTypeInformation -Encoding UTF8`, \
or write_file the CSV text. Default location: Documents.\n\
4. Verify by read_file (first lines) and report the path, row count and any rows you had to skip.",
    ),
    (
        "browser-teamwork",
        "Split a job between this PC and Kudu Connect, the browser agent (web + local in one task)",
        "1. Web parts (sites, forms, logged-in pages, extraction) → delegate_to_browser with a self-contained \
instruction, the URL, what 'done' means, and any data it needs in `context` (it can't read the PC).\n\
2. Independent parts run in parallel: delegate_to_browser(background=true), keep working locally, then \
check_browser_task.\n\
3. Feed results across: put what the browser returned into files/commands here, or back into the next browser task.\n\
4. Report the combined result, saying which side did what.",
    ),
];

fn user_skills_dir() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("mouse-ai").join("skills"))
}

/// `---\nname: x\ndescription: y\n---\nbody` → (name, description, body).
fn parse_skill(text: &str) -> (Option<String>, Option<String>, String) {
    let t = text.trim_start_matches('\u{feff}');
    if let Some(rest) = t.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            let (front, body) = rest.split_at(end);
            let mut name = None;
            let mut desc = None;
            for line in front.lines() {
                if let Some(v) = line.strip_prefix("name:") {
                    name = Some(v.trim().trim_matches('"').to_string());
                } else if let Some(v) = line.strip_prefix("description:") {
                    desc = Some(v.trim().trim_matches('"').to_string());
                }
            }
            return (name, desc, body.trim_start_matches("\n---").trim().to_string());
        }
    }
    (None, None, t.trim().to_string())
}

/// Built-in skills plus the user's own (user wins on a name clash).
pub fn all() -> Vec<Skill> {
    let mut out: Vec<Skill> = BUILTIN
        .iter()
        .map(|(n, d, b)| Skill { name: n.to_string(), description: d.to_string(), body: b.to_string(), user: false })
        .collect();
    let Some(dir) = user_skills_dir() else { return out };
    let Ok(entries) = std::fs::read_dir(&dir) else { return out };
    for e in entries.flatten() {
        let p = e.path();
        let (file, fallback) = if p.is_dir() {
            (p.join("SKILL.md"), p.file_name().map(|n| n.to_string_lossy().to_string()))
        } else if p.extension().map(|x| x == "md").unwrap_or(false) {
            (p.clone(), p.file_stem().map(|n| n.to_string_lossy().to_string()))
        } else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let (name, desc, body) = parse_skill(&text);
        let Some(name) = name.or(fallback) else { continue };
        if body.is_empty() {
            continue;
        }
        let description = desc.unwrap_or_else(|| body.lines().next().unwrap_or("").chars().take(120).collect());
        out.retain(|s| s.name != name);
        out.push(Skill { name, description, body: body.chars().take(12_000).collect(), user: true });
    }
    out
}

/// The short catalog that goes into the agent's opening context.
pub fn catalog_text() -> String {
    let skills = all();
    let mut s = String::from("[SKILLS - playbooks for common jobs. If the task matches one, call use_skill(name) FIRST and follow it]\n");
    for k in &skills {
        s.push_str(&format!("- {}: {}\n", k.name, k.description));
    }
    if let Some(dir) = user_skills_dir() {
        s.push_str(&format!("(The user can add their own skills as SKILL.md files in {}.)", dir.display()));
    }
    s
}

pub fn use_skill(args: &Value) -> Result<String, String> {
    let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_lowercase();
    let skills = all();
    match skills.iter().find(|s| s.name.to_lowercase() == name) {
        Some(s) => Ok(format!("[SKILL: {}{}]\n{}", s.name, if s.user { " (user)" } else { "" }, s.body)),
        None => Err(format!(
            "No skill named \"{}\". Available: {}",
            name,
            skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

pub fn tool_declarations() -> Vec<Value> {
    vec![
        json!({
            "name": "use_skill",
            "description": "Load the step-by-step playbook for a kind of job (see [SKILLS] in the task context). Call it before starting a matching task.",
            "parameters": { "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }
        }),
        json!({
            "name": "create_document",
            "description": "Create a real Word document (.docx) from Markdown: '# '/'## ' headings, '- ' and '1. ' lists, **bold**, *italic*, `code`, and | tables |. Use it whenever the user wants a document, report, docs or letter. Paths: absolute, or Documents\\Name.docx / Desktop\\Name.docx for the user's real folders. Returns the absolute path and size.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Where to save, ending in .docx" },
                    "markdown": { "type": "string", "description": "The full document content in Markdown" }
                },
                "required": ["path", "markdown"]
            }
        }),
    ]
}

// ── Markdown → .docx ─────────────────────────────────────────────────────────

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Inline Markdown → runs: **bold**, *italic* / _italic_, `code`.
fn runs(text: &str) -> String {
    let mut out = String::new();
    let (mut bold, mut italic) = (false, false);
    let chars: Vec<char> = text.chars().collect();
    let mut buf = String::new();
    let flush = |buf: &mut String, out: &mut String, bold: bool, italic: bool, code: bool| {
        if buf.is_empty() {
            return;
        }
        let mut rpr = String::new();
        if bold {
            rpr.push_str("<w:b/>");
        }
        if italic {
            rpr.push_str("<w:i/>");
        }
        if code {
            rpr.push_str("<w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\"/><w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F2F2F2\"/>");
        }
        out.push_str(&format!(
            "<w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
            if rpr.is_empty() { String::new() } else { format!("<w:rPr>{}</w:rPr>", rpr) },
            esc(buf)
        ));
        buf.clear();
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|x| *x == '`') {
                flush(&mut buf, &mut out, bold, italic, false);
                buf = chars[i + 1..i + 1 + end].iter().collect();
                flush(&mut buf, &mut out, bold, italic, true);
                i += end + 2;
                continue;
            }
        }
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            flush(&mut buf, &mut out, bold, italic, false);
            bold = !bold;
            i += 2;
            continue;
        }
        if (c == '*' || c == '_') && !(c == '_' && i > 0 && chars[i - 1].is_alphanumeric()) {
            flush(&mut buf, &mut out, bold, italic, false);
            italic = !italic;
            i += 1;
            continue;
        }
        buf.push(c);
        i += 1;
    }
    flush(&mut buf, &mut out, bold, italic, false);
    out
}

fn para(style: Option<&str>, content: &str, indent_twips: Option<u32>) -> String {
    let mut ppr = String::new();
    if let Some(s) = style {
        ppr.push_str(&format!("<w:pStyle w:val=\"{}\"/>", s));
    }
    if let Some(ind) = indent_twips {
        ppr.push_str(&format!("<w:ind w:left=\"{}\" w:hanging=\"300\"/>", ind));
    }
    format!("<w:p>{}{}</w:p>", if ppr.is_empty() { String::new() } else { format!("<w:pPr>{}</w:pPr>", ppr) }, content)
}

fn table(rows: &[Vec<String>]) -> String {
    let border = "<w:top w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/><w:left w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/><w:bottom w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/><w:right w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/><w:insideH w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/><w:insideV w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/>";
    let mut out = format!("<w:tbl><w:tblPr><w:tblW w:w=\"5000\" w:type=\"pct\"/><w:tblBorders>{}</w:tblBorders></w:tblPr>", border);
    for (ri, row) in rows.iter().enumerate() {
        out.push_str("<w:tr>");
        for cell in row {
            let content = if ri == 0 { runs(&format!("**{}**", cell.trim())) } else { runs(cell.trim()) };
            let shade = if ri == 0 { "<w:tcPr><w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"EAF0FB\"/></w:tcPr>" } else { "" };
            out.push_str(&format!("<w:tc>{}<w:p>{}</w:p></w:tc>", shade, content));
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl><w:p/>");
    out
}

fn body_xml(markdown: &str) -> String {
    let mut out = String::new();
    let lines: Vec<&str> = markdown.lines().collect();
    let mut i = 0;
    let mut in_code = false;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim_end();
        if t.trim_start().starts_with("```") {
            in_code = !in_code;
            i += 1;
            continue;
        }
        if in_code {
            out.push_str(&para(Some("Code"), &format!("<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r>", esc(t)), None));
            i += 1;
            continue;
        }
        let s = t.trim_start();
        if s.starts_with('|') {
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('|') {
                let r = lines[i].trim().trim_matches('|');
                let cells: Vec<String> = r.split('|').map(|c| c.trim().to_string()).collect();
                let is_rule = cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' '));
                if !is_rule {
                    rows.push(cells);
                }
                i += 1;
            }
            out.push_str(&table(&rows));
            continue;
        }
        if s.is_empty() {
            i += 1;
            continue;
        }
        let hashes = s.chars().take_while(|c| *c == '#').count();
        if hashes > 0 && s.chars().nth(hashes) == Some(' ') {
            let style = match hashes {
                1 => "Title",
                2 => "Heading1",
                3 => "Heading2",
                _ => "Heading3",
            };
            out.push_str(&para(Some(style), &runs(s[hashes + 1..].trim()), None));
        } else if s == "---" || s == "***" {
            out.push_str("<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:color=\"BFBFBF\"/></w:pBdr></w:pPr></w:p>");
        } else if let Some(rest) = s.strip_prefix("- ").or_else(|| s.strip_prefix("* ")).or_else(|| s.strip_prefix("• ")) {
            let depth = (line.len() - line.trim_start().len()) as u32 / 2;
            out.push_str(&para(None, &format!("<w:r><w:t xml:space=\"preserve\">•\t</w:t></w:r>{}", runs(rest)), Some(420 + depth * 360)));
        } else if let Some(pos) = s.find(". ").filter(|p| *p <= 3 && s[..*p].chars().all(|c| c.is_ascii_digit())) {
            let depth = (line.len() - line.trim_start().len()) as u32 / 2;
            out.push_str(&para(
                None,
                &format!("<w:r><w:t xml:space=\"preserve\">{}.\t</w:t></w:r>{}", &s[..pos], runs(&s[pos + 2..])),
                Some(420 + depth * 360),
            ));
        } else if let Some(q) = s.strip_prefix("> ") {
            out.push_str(&para(Some("Quote"), &runs(q), None));
        } else {
            out.push_str(&para(None, &runs(s), None));
        }
        i += 1;
    }
    out
}

const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Calibri" w:cs="Calibri"/><w:sz w:val="22"/><w:szCs w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="120" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="240"/></w:pPr><w:rPr><w:b/><w:color w:val="1F2A44"/><w:sz w:val="48"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="280" w:after="100"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:color w:val="2F4A8A"/><w:sz w:val="32"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="200" w:after="80"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:color w:val="2F4A8A"/><w:sz w:val="26"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:pPr><w:keepNext/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/><w:sz w:val="23"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Quote"><w:name w:val="Quote"/><w:basedOn w:val="Normal"/><w:pPr><w:ind w:left="567"/></w:pPr><w:rPr><w:i/><w:color w:val="555555"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Code"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="0"/></w:pPr><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas"/><w:sz w:val="19"/></w:rPr></w:style>
</w:styles>"#;

pub fn create_docx(markdown: &str) -> Result<Vec<u8>, String> {
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{}\
<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1134\" w:right=\"1134\" w:bottom=\"1134\" w:left=\"1134\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr>\
</w:body></w:document>",
        body_xml(markdown)
    );
    let files: [(&str, String); 5] = [
        ("[Content_Types].xml", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/><Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/></Types>".to_string()),
        ("_rels/.rels", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>".to_string()),
        ("word/_rels/document.xml.rels", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>".to_string()),
        ("word/styles.xml", STYLES.to_string()),
        ("word/document.xml", document),
    ];
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in files.iter() {
        w.start_file(*name, opts).map_err(|e| e.to_string())?;
        w.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
    }
    let cursor = w.finish().map_err(|e| e.to_string())?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docx_is_a_valid_zip_with_content() {
        let bytes = create_docx("# Cluely\nIntro **bold** and *it*.\n## Features\n- One\n- Two\n1. First\n| A | B |\n|---|---|\n| 1 | 2 |\n```\ncode\n```").unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut doc = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("word/document.xml").unwrap(), &mut doc).unwrap();
        assert!(doc.contains("w:val=\"Title\"") && doc.contains("Cluely") && doc.contains("<w:b/>") && doc.contains("<w:tbl>"));
    }

    /// Word refuses a file with malformed XML, so every part must parse.
    #[test]
    fn docx_parts_are_well_formed_xml() {
        let md = "# T & <Co>\n**a** *b* `c` _d_\n- x\n  - y\n2. z\n> q\n---\n| h1 | h2 |\n|:--|--:|\n| <1> | \"2\" |\n```rust\nlet x = 1 < 2;\n```";
        let bytes = create_docx(md).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).unwrap();
            let mut xml = String::new();
            std::io::Read::read_to_string(&mut f, &mut xml).unwrap();
            let mut reader = quick_xml::Reader::from_str(&xml);
            loop {
                match reader.read_event() {
                    Ok(quick_xml::events::Event::Eof) => break,
                    Ok(_) => {}
                    Err(e) => panic!("{} is not well-formed: {}", f.name(), e),
                }
            }
        }
    }

    #[test]
    fn frontmatter_is_parsed() {
        let (n, d, b) = parse_skill("---\nname: invoices\ndescription: Make invoices\n---\nStep 1");
        assert_eq!((n.as_deref(), d.as_deref(), b.as_str()), (Some("invoices"), Some("Make invoices"), "Step 1"));
    }

    #[test]
    fn builtin_skills_load() {
        assert!(use_skill(&json!({ "name": "word-document" })).unwrap().contains("create_document"));
        assert!(use_skill(&json!({ "name": "nope" })).is_err());
    }
}
