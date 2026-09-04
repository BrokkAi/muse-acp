//! Zed and JetBrains ACP settings installers.

use crate::json::{J, esc, parse_json};

const ZED_SETTINGS_REL: &str = ".config/zed/settings.json";
const INTELLIJ_SETTINGS_REL: &str = ".jetbrains/acp.json";
const DEFAULT_AGENT_NAME: &str = "muse-acp";
const DEFAULT_COMMAND: &str = "muse-acp";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Client {
    Zed,
    IntelliJ,
}

struct InstallerOpts {
    name: String,
    settings: Option<String>,
    command: String,
    env: Vec<(String, String)>,
    dry_run: bool,
    no_backup: bool,
}

impl InstallerOpts {
    fn new() -> Self {
        Self {
            name: DEFAULT_AGENT_NAME.to_string(),
            settings: None,
            command: DEFAULT_COMMAND.to_string(),
            env: Vec::new(),
            dry_run: false,
            no_backup: false,
        }
    }
}

fn usage() -> &'static str {
    "usage: muse-acp [command] [options]\n\
     \n\
     \x20 (no command)         run the ACP agent over stdio (what clients spawn)\n\
     \x20 install              register muse-acp as a Zed agent server\n\
     \x20 uninstall            remove the Zed settings entry again\n\
     \x20 install-intellij     register muse-acp in JetBrains IDEs\n\
     \x20 uninstall-intellij   remove the JetBrains settings entry\n\
     \x20 help [command]       show this help (-h/--help also work)\n\
     \x20 --version (-V)       print version\n\
     \n\
     install/install-intellij options:\n\
     \x20 --name <name>        agent_servers key (default: muse-acp)\n\
     \x20 --settings <path>    override the client settings file\n\
     \x20 --command <cmd>      agent executable (IntelliJ requires an absolute path)\n\
     \x20 --env KEY=VALUE      extra env for the agent entry (repeatable)\n\
     \x20 --dry-run            print planned changes without writing anything\n\
     \x20 --no-backup          do not write a .bak backup of settings.json\n\
     \n\
     uninstall/uninstall-intellij options:\n\
     \x20 --name, --settings, --dry-run, --no-backup (as above)"
}

enum Cli {
    Serve,
    Install(Client, InstallerOpts),
    Uninstall(Client, InstallerOpts),
    Help,
    Version,
}

fn take_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    let arg = &args[*i];
    if let Some(v) = arg.strip_prefix(&format!("{flag}=")) {
        if v.is_empty() {
            return Err(format!("{flag} requires a value"));
        }
        return Ok(v.to_string());
    }
    *i += 1;
    if *i >= args.len() {
        return Err(format!("{flag} requires a value"));
    }
    Ok(args[*i].clone())
}

fn parse_installer_opts(
    args: &[String],
    i: &mut usize,
    is_install: bool,
) -> Result<InstallerOpts, String> {
    let cmd = if is_install { "install" } else { "uninstall" };
    let mut o = InstallerOpts::new();
    while *i < args.len() {
        let a = args[*i].clone();
        if a == "-h" || a == "--help" {
            return Err(format!("__help_{cmd}"));
        } else if a == "--name" || a.starts_with("--name=") {
            o.name = take_value(args, i, "--name")?;
            if o.name.trim().is_empty() {
                return Err("--name must not be empty".to_string());
            }
        } else if a == "--settings" || a.starts_with("--settings=") {
            o.settings = Some(take_value(args, i, "--settings")?);
        } else if a == "--command" || a.starts_with("--command=") {
            if !is_install {
                return Err("--command is only valid for install".to_string());
            }
            o.command = take_value(args, i, "--command")?;
            if o.command.trim().is_empty() {
                return Err("--command must not be empty".to_string());
            }
        } else if a == "--env" || a.starts_with("--env=") {
            if !is_install {
                return Err("--env is only valid for install".to_string());
            }
            let kv = take_value(args, i, "--env")?;
            let (k, v) = kv
                .split_once('=')
                .ok_or_else(|| "--env requires KEY=VALUE".to_string())?;
            if k.is_empty() {
                return Err("--env requires a non-empty key".to_string());
            }
            o.env.push((k.to_string(), v.to_string()));
        } else if a == "--dry-run" {
            o.dry_run = true;
        } else if a == "--no-backup" {
            o.no_backup = true;
        } else {
            return Err(format!("unexpected argument: {a}"));
        }
        *i += 1;
    }
    Ok(o)
}

fn parse_args(args: &[String]) -> Result<Cli, String> {
    if args.is_empty() {
        return Ok(Cli::Serve);
    }
    match args[0].as_str() {
        "-h" | "--help" | "help" => Ok(Cli::Help),
        "-V" | "--version" => {
            if args.len() > 1 {
                return Err(format!("unexpected argument: {}", args[1]));
            }
            Ok(Cli::Version)
        }
        "install" | "uninstall" | "install-intellij" | "uninstall-intellij" => {
            let is_install = args[0] == "install" || args[0] == "install-intellij";
            let client = if args[0].ends_with("-intellij") {
                Client::IntelliJ
            } else {
                Client::Zed
            };
            let mut i = 1;
            match parse_installer_opts(args, &mut i, is_install) {
                Ok(o) => Ok(if is_install {
                    Cli::Install(client, o)
                } else {
                    Cli::Uninstall(client, o)
                }),
                Err(e) if e == "__help_install" || e == "__help_uninstall" => Ok(Cli::Help),
                Err(e) => Err(e),
            }
        }
        other => Err(format!("unexpected argument: {other}")),
    }
}

// --- JSONC scanner (Zed settings allow comments + trailing commas) ---

#[derive(Debug)]
struct JsoncEntry {
    key: String,
    key_start: usize,
    value_start: usize,
    value_end: usize,
    comma_after: Option<usize>,
}

#[derive(Debug)]
struct JsoncObject {
    #[allow(dead_code)]
    open: usize,
    close: usize,
    entries: Vec<JsoncEntry>,
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

fn skip_trivia(b: &[u8], mut p: usize) -> usize {
    while p < b.len() {
        if is_ws(b[p]) {
            p += 1;
        } else if b[p] == b'/' && p + 1 < b.len() && b[p + 1] == b'/' {
            p += 2;
            while p < b.len() && b[p] != b'\n' {
                p += 1;
            }
        } else if b[p] == b'/' && p + 1 < b.len() && b[p + 1] == b'*' {
            p += 2;
            while p + 1 < b.len() && !(b[p] == b'*' && b[p + 1] == b'/') {
                p += 1;
            }
            p = (p + 2).min(b.len());
        } else {
            break;
        }
    }
    p
}

fn is_trivia_only(s: &str) -> bool {
    skip_trivia(s.as_bytes(), 0) == s.len()
}

fn slice_is_ws_only(s: &str) -> bool {
    s.bytes().all(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r'))
}

fn parse_jsonc_string(b: &[u8], p: usize) -> Option<(String, usize)> {
    if b.get(p) != Some(&b'"') {
        return None;
    }
    let mut out = String::new();
    let mut i = p + 1;
    while i < b.len() {
        match b[i] {
            b'"' => return Some((out, i + 1)),
            b'\\' => {
                i += 1;
                if i >= b.len() {
                    return None;
                }
                match b[i] {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000C}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        if i + 4 >= b.len() {
                            return None;
                        }
                        let hex = std::str::from_utf8(&b[i + 1..i + 5]).ok()?;
                        let cp = u32::from_str_radix(hex, 16).ok()?;
                        // BMP only; astral keys compare by replacement char (fine:
                        // we only compare ASCII keys like "agent_servers").
                        out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                        i += 4;
                    }
                    _ => return None,
                }
                i += 1;
            }
            _ => {
                let s = std::str::from_utf8(&b[i..]).ok()?;
                let c = s.chars().next()?;
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    None
}

fn scan_jsonc_value_end(b: &[u8], p: usize) -> Option<usize> {
    let c = *b.get(p)?;
    match c {
        b'"' => Some(parse_jsonc_string(b, p)?.1),
        b'{' | b'[' => {
            let close = if c == b'{' { b'}' } else { b']' };
            let mut depth = 0usize;
            let mut i = p;
            while i < b.len() {
                if b[i] == b'"' {
                    i = parse_jsonc_string(b, i)?.1;
                    continue;
                }
                if b[i] == b'/' && i + 1 < b.len() && (b[i + 1] == b'/' || b[i + 1] == b'*') {
                    i = skip_trivia(b, i);
                    continue;
                }
                if b[i] == c {
                    depth += 1;
                } else if b[i] == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                i += 1;
            }
            None
        }
        _ => {
            let mut i = p;
            while i < b.len() && !matches!(b[i], b',' | b'}' | b']') {
                i += 1;
            }
            while i > p && is_ws(b[i - 1]) {
                i -= 1;
            }
            if i == p { None } else { Some(i) }
        }
    }
}

fn parse_jsonc_object(b: &[u8], open: usize) -> Option<JsoncObject> {
    if b.get(open) != Some(&b'{') {
        return None;
    }
    let mut entries = Vec::new();
    let mut p = skip_trivia(b, open + 1);
    if b.get(p) == Some(&b'}') {
        return Some(JsoncObject {
            open,
            close: p,
            entries,
        });
    }
    loop {
        let (key, key_end) = parse_jsonc_string(b, p)?;
        let key_start = p;
        p = skip_trivia(b, key_end);
        if b.get(p) != Some(&b':') {
            return None;
        }
        p = skip_trivia(b, p + 1);
        let value_start = p;
        let value_end = scan_jsonc_value_end(b, p)?;
        p = skip_trivia(b, value_end);
        let comma_after = if b.get(p) == Some(&b',') {
            p += 1;
            Some(p - 1)
        } else {
            None
        };
        entries.push(JsoncEntry {
            key,
            key_start,
            value_start,
            value_end,
            comma_after,
        });
        p = skip_trivia(b, p);
        match b.get(p) {
            Some(&b'}') => {
                return Some(JsoncObject {
                    open,
                    close: p,
                    entries,
                });
            }
            Some(&b'"') => {}
            _ => return None,
        }
    }
}

/// Leading whitespace of the line containing `pos` (for matching indent style).
fn line_indent(text: &str, pos: usize) -> String {
    let b = text.as_bytes();
    let mut s = pos.min(b.len());
    while s > 0 && b[s - 1] != b'\n' {
        s -= 1;
    }
    let mut indent = String::new();
    for &c in &b[s..pos.min(b.len())] {
        if c == b' ' || c == b'\t' {
            indent.push(c as char);
        } else {
            break;
        }
    }
    indent
}

fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            match parse_jsonc_string(b, i) {
                Some((_, end)) => {
                    out.push_str(&src[i..end]);
                    i = end;
                }
                None => {
                    out.push_str(&src[i..]);
                    break;
                }
            }
        } else if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            i += 2;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                if b[i] == b'\n' {
                    out.push('\n');
                }
                i += 1;
            }
            i = (i + 2).min(b.len());
        } else {
            let c = src[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

fn strip_trailing_commas(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            match parse_jsonc_string(b, i) {
                Some((_, end)) => {
                    out.push_str(&src[i..end]);
                    i = end;
                }
                None => {
                    out.push_str(&src[i..]);
                    break;
                }
            }
        } else if b[i] == b',' {
            let mut j = i + 1;
            while j < b.len() && is_ws(b[j]) {
                j += 1;
            }
            if j < b.len() && (b[j] == b'}' || b[j] == b']') {
                i += 1; // drop trailing comma
            } else {
                out.push(',');
                i += 1;
            }
        } else {
            let c = src[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

fn check_valid_jsonc_object(text: &str) -> bool {
    matches!(
        parse_json(strip_trailing_commas(&strip_comments(text)).trim()),
        Ok(J::Obj(_))
    )
}

fn render_agent_value(
    client: Client,
    command: &str,
    env: &[(String, String)],
    fi: &str,
    ci: &str,
) -> String {
    let mut s = String::from("{\n");
    if client == Client::Zed {
        s.push_str(&format!("{fi}\"type\": \"custom\",\n"));
    }
    s.push_str(&format!("{fi}\"command\": {},\n", esc(command)));
    s.push_str(&format!("{fi}\"args\": [],\n"));
    if env.is_empty() {
        s.push_str(&format!("{fi}\"env\": {{}}\n"));
    } else {
        s.push_str(&format!("{fi}\"env\": {{\n"));
        for (i, (k, v)) in env.iter().enumerate() {
            s.push_str(&format!("{fi}  {}: {}", esc(k), esc(v)));
            if i + 1 < env.len() {
                s.push(',');
            }
            s.push('\n');
        }
        s.push_str(&format!("{fi}}}\n"));
    }
    s.push_str(ci);
    s.push('}');
    s
}

fn render_agent_entry(
    client: Client,
    name: &str,
    command: &str,
    env: &[(String, String)],
    ki: &str,
) -> String {
    let fi = format!("{ki}  ");
    format!(
        "{}{}: {}",
        ki,
        esc(name),
        render_agent_value(client, command, env, &fi, ki)
    )
}

#[derive(Debug, PartialEq)]
enum EditOutcome {
    Added,
    Updated,
}

fn install_settings_edit(
    original: &str,
    name: &str,
    command: &str,
    env: &[(String, String)],
    client: Client,
) -> Result<(String, EditOutcome), String> {
    if is_trivia_only(original) {
        let entry = render_agent_entry(client, name, command, env, "    ");
        let fresh = format!("{{\n  \"agent_servers\": {{\n{entry}\n  }}\n}}\n");
        if !check_valid_jsonc_object(&fresh) {
            return Err("internal error: generated invalid settings".to_string());
        }
        return Ok((fresh, EditOutcome::Added));
    }
    if !check_valid_jsonc_object(original) {
        return Err("existing settings file is not valid JSON/JSONC; fix it manually or pass --settings <path>".to_string());
    }
    let b = original.as_bytes();
    let root = parse_jsonc_object(b, skip_trivia(b, 0))
        .ok_or_else(|| "settings file root is not a JSON object; refusing to edit".to_string())?;
    let aservers = root.entries.iter().find(|e| e.key == "agent_servers");
    let Some(aservers) = aservers else {
        // Insert a new top-level "agent_servers" key before the closing brace.
        let ind = root
            .entries
            .last()
            .map(|e| line_indent(original, e.key_start))
            .unwrap_or_else(|| "  ".to_string());
        let entry = render_agent_entry(client, name, command, env, &format!("{ind}  "));
        let block = format!("\n{ind}\"agent_servers\": {{\n{entry}\n{ind}}}");
        let (ins, prefix) = match root.entries.last() {
            None => (root.close, String::new()), // empty (maybe commented) object
            Some(last) => (
                last.comma_after.map(|c| c + 1).unwrap_or(last.value_end),
                if last.comma_after.is_some() { "" } else { "," }.to_string(),
            ),
        };
        let mut out = String::with_capacity(original.len() + block.len() + 2);
        out.push_str(&original[..ins]);
        out.push_str(&prefix);
        out.push_str(&block);
        if root.entries.is_empty() {
            out.push('\n');
        }
        out.push_str(&original[ins..]);
        if !check_valid_jsonc_object(&out) {
            return Err(
                "internal error: produced invalid settings; file left untouched".to_string(),
            );
        }
        return Ok((out, EditOutcome::Added));
    };
    let sub = parse_jsonc_object(b, aservers.value_start).ok_or_else(|| {
        "\"agent_servers\" exists but is not an object; remove or fix it manually".to_string()
    })?;
    let ai_ind = line_indent(original, aservers.key_start);
    if let Some(existing) = sub.entries.iter().find(|e| e.key == name) {
        let ki = line_indent(original, existing.key_start);
        let fi = format!("{ki}  ");
        let value = render_agent_value(client, command, env, &fi, &ki);
        let mut out = String::with_capacity(original.len() + value.len());
        out.push_str(&original[..existing.value_start]);
        out.push_str(&value);
        out.push_str(&original[existing.value_end..]);
        if !check_valid_jsonc_object(&out) {
            return Err(
                "internal error: produced invalid settings; file left untouched".to_string(),
            );
        }
        return Ok((out, EditOutcome::Updated));
    }
    // Insert a new entry into the existing agent_servers object.
    let entry_ind = format!("{ai_ind}  ");
    let entry = render_agent_entry(client, name, command, env, &entry_ind);
    let mut out = String::with_capacity(original.len() + entry.len() + 8);
    if sub.entries.is_empty() {
        if slice_is_ws_only(&original[aservers.value_start + 1..sub.close]) {
            out.push_str(&original[..aservers.value_start]);
            out.push_str(&format!("{{\n{entry}\n{ai_ind}}}"));
            out.push_str(&original[sub.close + 1..]);
        } else {
            // Non-empty trivia (comments): keep it, append after, re-indent close.
            out.push_str(&original[..sub.close]);
            out.push_str(&format!("\n{entry}\n{ai_ind}}}"));
            out.push_str(&original[sub.close + 1..]);
        }
    } else {
        let last = sub.entries.last().unwrap();
        let ins = last.comma_after.map(|c| c + 1).unwrap_or(last.value_end);
        let prefix = if last.comma_after.is_some() { "" } else { "," };
        out.push_str(&original[..ins]);
        out.push_str(prefix);
        out.push_str(&format!("\n{entry}"));
        out.push_str(&original[ins..]);
    }
    if !check_valid_jsonc_object(&out) {
        return Err("internal error: produced invalid settings; file left untouched".to_string());
    }
    Ok((out, EditOutcome::Added))
}

/// Span covering an object entry plus one adjacent comma, so removal leaves
/// valid JSON.
fn entry_removal_span(obj: &JsoncObject, idx: usize) -> (usize, usize) {
    let e = &obj.entries[idx];
    if let Some(c) = e.comma_after {
        return (e.key_start, c + 1);
    }
    if idx > 0 {
        let prev = &obj.entries[idx - 1];
        return (prev.comma_after.unwrap_or(prev.value_end), e.value_end);
    }
    (e.key_start, e.value_end)
}

fn remove_entry_at(text: &str, obj: &JsoncObject, idx: usize) -> String {
    let (rs, re) = entry_removal_span(obj, idx);
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..rs]);
    out.push_str(&text[re..]);
    out
}

fn uninstall_settings_edit(original: &str, name: &str) -> Result<(String, bool), String> {
    if is_trivia_only(original) {
        return Ok((original.to_string(), false));
    }
    if !check_valid_jsonc_object(original) {
        return Err("existing settings file is not valid JSON/JSONC; fix it manually or pass --settings <path>".to_string());
    }
    let b = original.as_bytes();
    let root = parse_jsonc_object(b, skip_trivia(b, 0))
        .ok_or_else(|| "settings file root is not a JSON object; refusing to edit".to_string())?;
    let Some(ai) = root.entries.iter().position(|e| e.key == "agent_servers") else {
        return Ok((original.to_string(), false));
    };
    let sub = parse_jsonc_object(b, root.entries[ai].value_start).ok_or_else(|| {
        "\"agent_servers\" exists but is not an object; remove or fix it manually".to_string()
    })?;
    let Some(idx) = sub.entries.iter().position(|e| e.key == name) else {
        return Ok((original.to_string(), false));
    };
    let mut out = remove_entry_at(original, &sub, idx);
    // Drop agent_servers itself when it is left empty.
    let b2 = out.clone();
    let bb = b2.as_bytes();
    if let Some(root2) = parse_jsonc_object(bb, skip_trivia(bb, 0))
        && let Some(ai2) = root2.entries.iter().position(|e| e.key == "agent_servers")
    {
        let v = &root2.entries[ai2];
        if let Some(sub2) = parse_jsonc_object(bb, v.value_start)
            && sub2.entries.is_empty()
        {
            out = remove_entry_at(&out, &root2, ai2);
        }
    }
    // Normalize a fully-emptied file instead of leaving a whitespace shell.
    let compact: String = out.chars().filter(|c| !c.is_whitespace()).collect();
    if compact == "{}" {
        out = "{}\n".to_string();
    }
    if !check_valid_jsonc_object(&out) {
        return Err("internal error: produced invalid settings; file left untouched".to_string());
    }
    Ok((out, true))
}

// --- paths + binary install ---

fn home_dir() -> Result<std::path::PathBuf, String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .map_err(|_| {
            "cannot determine home directory (HOME/USERPROFILE unset); pass --settings <path>"
                .to_string()
        })
}

fn default_settings_path(client: Client) -> Result<std::path::PathBuf, String> {
    let relative = match client {
        Client::Zed => ZED_SETTINGS_REL,
        Client::IntelliJ => INTELLIJ_SETTINGS_REL,
    };
    Ok(home_dir()?.join(relative))
}

fn intellij_command(command: &str) -> Result<String, String> {
    let path = if command == DEFAULT_COMMAND {
        std::env::current_exe()
            .map_err(|e| format!("cannot determine the muse-acp executable path: {e}"))?
    } else {
        let path = std::path::PathBuf::from(command);
        if !path.is_absolute() {
            return Err(format!(
                "IntelliJ requires a full executable path; --command is not absolute: {command}"
            ));
        }
        path
    };
    if !path.is_file() {
        return Err(format!(
            "IntelliJ agent executable does not exist or is not a file: {}",
            path.display()
        ));
    }
    path.into_os_string()
        .into_string()
        .map_err(|_| "IntelliJ agent executable path is not valid UTF-8".to_string())
}

fn write_backup(path: &std::path::Path) {
    let mut bak = path.as_os_str().to_owned();
    bak.push(".bak");
    let bak_path = std::path::Path::new(&bak);
    match std::fs::copy(path, bak_path) {
        Ok(_) => println!("muse-acp: backup: {}", bak_path.display()),
        Err(e) => eprintln!(
            "muse-acp: warning: cannot write backup {}: {e}",
            bak_path.display()
        ),
    }
}

fn cmd_install(client: Client, o: &InstallerOpts) -> i32 {
    let settings_path = match &o.settings {
        Some(s) => std::path::PathBuf::from(s),
        None => match default_settings_path(client) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("muse-acp: {e}");
                return 1;
            }
        },
    };
    let command = match client {
        // Zed resolves the registered command through PATH at spawn time.
        Client::Zed => o.command.clone(),
        // JetBrains requires a full path in ~/.jetbrains/acp.json.
        Client::IntelliJ => match intellij_command(&o.command) {
            Ok(command) => command,
            Err(e) => {
                eprintln!("muse-acp: {e}");
                return 1;
            }
        },
    };
    let original = match std::fs::read_to_string(&settings_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            eprintln!("muse-acp: cannot read {}: {e}", settings_path.display());
            return 1;
        }
    };
    let (updated, outcome) =
        match install_settings_edit(&original, &o.name, &command, &o.env, client) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("muse-acp: {e}");
                return 1;
            }
        };
    let action = if outcome == EditOutcome::Updated {
        "update"
    } else {
        "add"
    };
    if o.dry_run {
        println!("muse-acp: dry run — nothing written");
        println!(
            "muse-acp: would {action} entry \"{}\" in {} (command: {command})",
            o.name,
            settings_path.display()
        );
        return 0;
    }
    if !original.is_empty() && !o.no_backup {
        write_backup(&settings_path);
    }
    if let Some(parent) = settings_path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("muse-acp: cannot create {}: {e}", parent.display());
        return 1;
    }
    if let Err(e) = std::fs::write(&settings_path, &updated) {
        eprintln!("muse-acp: cannot write {}: {e}", settings_path.display());
        return 1;
    }
    println!(
        "muse-acp: {} entry \"{}\" in {} (command: {command})",
        if outcome == EditOutcome::Updated {
            "updated"
        } else {
            "added"
        },
        o.name,
        settings_path.display()
    );
    match client {
        Client::Zed => println!(
            "muse-acp: restart Zed (or reload settings) and select \"{}\" in the Agent panel.",
            o.name
        ),
        Client::IntelliJ => println!(
            "muse-acp: open AI Chat in your JetBrains IDE and select \"{}\" as the agent.",
            o.name
        ),
    }
    0
}

fn cmd_uninstall(client: Client, o: &InstallerOpts) -> i32 {
    let settings_path = match &o.settings {
        Some(s) => std::path::PathBuf::from(s),
        None => match default_settings_path(client) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("muse-acp: {e}");
                return 1;
            }
        },
    };
    let original = match std::fs::read_to_string(&settings_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "muse-acp: nothing to do (no settings file at {})",
                settings_path.display()
            );
            return 0;
        }
        Err(e) => {
            eprintln!("muse-acp: cannot read {}: {e}", settings_path.display());
            return 1;
        }
    };
    let (updated, removed) = match uninstall_settings_edit(&original, &o.name) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("muse-acp: {e}");
            return 1;
        }
    };
    if !removed {
        println!(
            "muse-acp: nothing to do (no \"{}\" entry in {})",
            o.name,
            settings_path.display()
        );
        return 0;
    }
    if o.dry_run {
        println!("muse-acp: dry run — nothing written");
        println!(
            "muse-acp: would remove entry \"{}\" from {}",
            o.name,
            settings_path.display()
        );
        return 0;
    }
    if !o.no_backup {
        write_backup(&settings_path);
    }
    if let Err(e) = std::fs::write(&settings_path, &updated) {
        eprintln!("muse-acp: cannot write {}: {e}", settings_path.display());
        return 1;
    }
    println!(
        "muse-acp: removed entry \"{}\" from {}",
        o.name,
        settings_path.display()
    );
    0
}

pub fn dispatch(args: &[String]) -> Option<i32> {
    match parse_args(args) {
        Ok(Cli::Serve) => None,
        Ok(Cli::Install(client, options)) => Some(cmd_install(client, &options)),
        Ok(Cli::Uninstall(client, options)) => Some(cmd_uninstall(client, &options)),
        Ok(Cli::Help) => {
            println!(
                "muse-acp {} — Rust ACP-to-MSP adapter for Muse Code",
                env!("CARGO_PKG_VERSION")
            );
            println!();
            println!("{}", usage());
            Some(0)
        }
        Ok(Cli::Version) => {
            println!("muse-acp {}", env!("CARGO_PKG_VERSION"));
            Some(0)
        }
        Err(error) => {
            eprintln!("muse-acp: {error}");
            eprintln!("{}", usage());
            Some(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env1() -> Vec<(String, String)> {
        vec![("FOO".to_string(), "bar".to_string())]
    }

    #[test]
    fn fresh_file_creates_agent_servers() {
        let (out, outcome) =
            install_settings_edit("", "muse-acp", "/bin/muse-acp", &[], Client::Zed).unwrap();
        assert_eq!(outcome, EditOutcome::Added);
        assert!(check_valid_jsonc_object(&out));
        assert!(out.contains("\"agent_servers\""));
        assert!(out.contains("\"command\": \"/bin/muse-acp\""));
        assert!(out.contains("\"type\": \"custom\""));
    }

    #[test]
    fn intellij_entry_uses_its_native_shape() {
        let (out, outcome) =
            install_settings_edit("", "muse-acp", "/opt/muse-acp", &env1(), Client::IntelliJ)
                .unwrap();
        assert_eq!(outcome, EditOutcome::Added);
        assert!(out.contains("\"command\": \"/opt/muse-acp\""));
        assert!(out.contains("\"args\": []"));
        assert!(out.contains("\"FOO\": \"bar\""));
        assert!(!out.contains("\"type\""));
        assert!(check_valid_jsonc_object(&out));
    }

    #[test]
    fn intellij_default_command_is_an_absolute_existing_executable() {
        let command = intellij_command(DEFAULT_COMMAND).unwrap();
        let path = std::path::Path::new(&command);
        assert!(path.is_absolute());
        assert!(path.is_file());
        assert!(intellij_command("relative/muse-acp").is_err());
    }

    #[test]
    fn preserves_comments_and_other_keys() {
        let original = "{\n  // theme comment\n  \"theme\": \"One Dark\",\n  /* block\n     comment */\n  \"tab_size\": 4,\n}\n";
        let (out, _) =
            install_settings_edit(original, "muse-acp", "/bin/muse-acp", &env1(), Client::Zed)
                .unwrap();
        assert!(out.contains("// theme comment"));
        assert!(out.contains("/* block\n     comment */"));
        assert!(out.contains("\"theme\": \"One Dark\""));
        assert!(out.contains("\"FOO\": \"bar\""));
        assert!(check_valid_jsonc_object(&out));
    }

    #[test]
    fn install_is_idempotent() {
        let original = "{\n  \"theme\": \"x\",\n}\n";
        let (once, _) =
            install_settings_edit(original, "muse-acp", "/bin/muse-acp", &[], Client::Zed).unwrap();
        let (twice, outcome) =
            install_settings_edit(&once, "muse-acp", "/bin/muse-acp", &[], Client::Zed).unwrap();
        assert_eq!(outcome, EditOutcome::Updated);
        assert_eq!(once, twice);
    }

    #[test]
    fn replaces_existing_entry_keeps_siblings() {
        let original = "{\n  \"agent_servers\": {\n    \"other\": {\n      \"type\": \"custom\",\n      \"command\": \"other-bin\"\n    },\n    \"muse-acp\": {\n      \"type\": \"custom\",\n      \"command\": \"/old/path\"\n    }\n  }\n}\n";
        let (out, outcome) =
            install_settings_edit(original, "muse-acp", "/new/path", &[], Client::Zed).unwrap();
        assert_eq!(outcome, EditOutcome::Updated);
        assert!(out.contains("\"command\": \"/new/path\""));
        assert!(!out.contains("/old/path"));
        assert!(out.contains("\"other\""));
        assert!(out.contains("other-bin"));
        assert!(check_valid_jsonc_object(&out));
    }

    #[test]
    fn handles_trailing_commas() {
        let original = "{\n  \"agent_servers\": {\n    \"other\": {\"command\": \"x\",},\n  },\n  \"theme\": \"y\",\n}\n";
        let (out, _) =
            install_settings_edit(original, "muse-acp", "/bin/muse-acp", &[], Client::Zed).unwrap();
        assert!(out.contains("\"other\""));
        assert!(out.contains("\"muse-acp\""));
        assert!(check_valid_jsonc_object(&out));
    }

    #[test]
    fn rejects_non_object_root_and_agent_servers() {
        assert!(install_settings_edit("[1,2]", "muse-acp", "/b", &[], Client::Zed).is_err());
        let bad = "{ \"agent_servers\": null }";
        assert!(install_settings_edit(bad, "muse-acp", "/b", &[], Client::Zed).is_err());
    }

    #[test]
    fn uninstall_removes_entry_and_empty_parent() {
        let original = "{\n  // keep me\n  \"theme\": \"x\",\n  \"agent_servers\": {\n    \"muse-acp\": {\n      \"type\": \"custom\",\n      \"command\": \"/bin/muse-acp\"\n    }\n  }\n}\n";
        let (out, removed) = uninstall_settings_edit(original, "muse-acp").unwrap();
        assert!(removed);
        assert!(!out.contains("muse-acp"));
        assert!(!out.contains("agent_servers"));
        assert!(out.contains("// keep me"));
        assert!(out.contains("\"theme\": \"x\""));
        assert!(check_valid_jsonc_object(&out));
    }

    #[test]
    fn uninstall_keeps_siblings() {
        let original = "{\n  \"agent_servers\": {\n    \"other\": {\"command\": \"x\"},\n    \"muse-acp\": {\"command\": \"y\"}\n  }\n}\n";
        let (out, removed) = uninstall_settings_edit(original, "muse-acp").unwrap();
        assert!(removed);
        assert!(!out.contains("muse-acp"));
        assert!(out.contains("\"agent_servers\""));
        assert!(out.contains("\"other\""));
        assert!(check_valid_jsonc_object(&out));
    }

    #[test]
    fn uninstall_last_entry_collapses_file() {
        let (out, _) = install_settings_edit("", "muse-acp", "muse-acp", &[], Client::Zed).unwrap();
        let (out, removed) = uninstall_settings_edit(&out, "muse-acp").unwrap();
        assert!(removed);
        assert_eq!(out, "{}\n");
    }

    #[test]
    fn uninstall_missing_entry_is_noop() {
        let original = "{ \"theme\": \"x\" }\n";
        let (out, removed) = uninstall_settings_edit(original, "muse-acp").unwrap();
        assert!(!removed);
        assert_eq!(out, original);
    }

    #[test]
    fn arg_parsing() {
        assert!(matches!(parse_args(&[]).unwrap(), Cli::Serve));
        assert!(matches!(parse_args(&["help".into()]).unwrap(), Cli::Help));
        let args = vec![
            "install".into(),
            "--name=n".into(),
            "--env".into(),
            "A=B".into(),
            "--dry-run".into(),
        ];
        match parse_args(&args).unwrap() {
            Cli::Install(Client::Zed, o) => {
                assert_eq!(o.name, "n");
                assert_eq!(o.command, "muse-acp");
                assert_eq!(o.env, vec![("A".to_string(), "B".to_string())]);
                assert!(o.dry_run);
            }
            _ => panic!("expected install"),
        }
        match parse_args(&["install".into(), "--command=/x/y".into()]).unwrap() {
            Cli::Install(Client::Zed, o) => assert_eq!(o.command, "/x/y"),
            _ => panic!("expected install"),
        }
        assert!(matches!(
            parse_args(&["install-intellij".into()]).unwrap(),
            Cli::Install(Client::IntelliJ, _)
        ));
        assert!(matches!(
            parse_args(&["uninstall-intellij".into()]).unwrap(),
            Cli::Uninstall(Client::IntelliJ, _)
        ));
        assert!(parse_args(&["install".into(), "--bogus".into()]).is_err());
        assert!(parse_args(&["uninstall".into(), "--env".into(), "A=B".into()]).is_err());
        assert!(parse_args(&["frobnicate".into()]).is_err());
    }
}
