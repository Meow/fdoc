//! Renders the documentation model to HTML pages.

use std::fmt::Write;

use crate::doc::{summary_of, DocBlock, Environment, Realm, Return};
use crate::html::{esc, json_str};
use crate::markdown;
use crate::model::{Function, FunctionKind, Group, Module, ModuleKind, Project};
use crate::scanner::Sep;

pub struct Page {
    /// Path relative to the output root, `/` separated.
    pub path: String,
    pub content: String,
}

const STYLE: &str = include_str!("../assets/style.css");
const APP: &str = include_str!("../assets/app.js");

const LINK_ICON: &str = r##"<svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><path fill="currentColor" d="M3.9 12c0-1.71 1.39-3.1 3.1-3.1h4V7H7c-2.76 0-5 2.24-5 5s2.24 5 5 5h4v-1.9H7c-1.71 0-3.1-1.39-3.1-3.1zM8 13h8v-2H8v2zm9-6h-4v1.9h4c1.71 0 3.1 1.39 3.1 3.1s-1.39 3.1-3.1 3.1h-4V17h4c2.76 0 5-2.24 5-5s-2.24-5-5-5z"/></svg>"##;
const MOON_ICON: &str = r##"<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path fill="currentColor" d="M12 3a9 9 0 1 0 9 9c0-.46-.04-.92-.1-1.36a5.389 5.389 0 0 1-4.4 2.26 5.403 5.403 0 0 1-3.14-9.8c-.44-.06-.9-.1-1.36-.1z"/></svg>"##;
const SUN_ICON: &str = r##"<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path fill="currentColor" d="M6.76 4.84l-1.8-1.79-1.41 1.41 1.79 1.79 1.42-1.41zM4 10.5H1v2h3v-2zm9-9.95h-2V3.5h2V.55zm7.45 3.91l-1.41-1.41-1.79 1.79 1.41 1.41 1.79-1.79zm-3.21 13.7l1.79 1.8 1.41-1.41-1.8-1.79-1.4 1.4zM20 10.5v2h3v-2h-3zm-8-5c-3.31 0-6 2.69-6 6s2.69 6 6 6 6-2.69 6-6-2.69-6-6-6zm-1 16.95h2V19.5h-2v2.95zm-7.45-3.91l1.41 1.41 1.79-1.8-1.41-1.41-1.79 1.8z"/></svg>"##;
const MENU_ICON: &str = r##"<svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true"><path fill="currentColor" d="M3 18h18v-2H3v2zm0-5h18v-2H3v2zm0-7v2h18V6H3z"/></svg>"##;

#[derive(Clone, Copy, Default)]
struct Active<'a> {
    group_dir: Option<&'a str>,
    module_slug: Option<&'a str>,
}

pub fn render_all(project: &Project) -> Vec<Page> {
    let mut pages = vec![
        Page { path: "assets/style.css".into(), content: STYLE.to_string() },
        Page { path: "assets/app.js".into(), content: APP.to_string() },
        Page { path: "assets/search_data.js".into(), content: search_data(project) },
        Page { path: "assets/sidebar_items.js".into(), content: sidebar_items(project) },
        Page { path: "index.html".into(), content: project_index(project) },
        Page { path: "search.html".into(), content: search_page(project) },
    ];

    for section in &project.sections {
        for group in &section.groups {
            if !group.is_core {
                pages.push(Page { path: format!("{}/index.html", group.dir), content: group_index(project, group) });
            }
            for module in &group.modules {
                pages.push(Page { path: format!("{}/{}.html", group.dir, module.slug), content: module_page(project, group, module) });
            }
        }
    }
    pages
}

fn root_for(path: &str) -> String {
    "../".repeat(path.matches('/').count())
}

fn project_label(project: &Project) -> String {
    match &project.version {
        Some(v) => format!("{} v{v}", project.title),
        None => project.title.clone(),
    }
}

fn layout(project: &Project, path: &str, title: &str, active: Active, body: &str) -> String {
    let root = root_for(path);
    let mut out = String::new();
    let _ = write!(
        out,
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="generator" content="fdoc">
<title>{title} — {label}</title>
<link rel="stylesheet" href="{root}assets/style.css">
<script>try{{var t=localStorage.getItem('fdoc-theme');if(t)document.documentElement.setAttribute('data-theme',t)}}catch(e){{}}</script>
</head>
<body data-root="{root}" data-group="{group}" data-module="{module}">
<div class="progress" id="progress"></div>
<button class="sidebar-toggle" type="button" aria-label="Toggle navigation">{menu}</button>
<div class="sidebar-backdrop"></div>
<aside class="sidebar" id="sidebar">
<div class="sidebar-header">
<a class="sidebar-project" href="{root}index.html"><span class="sidebar-project-name">{name}</span>{version}</a>
</div>
<div class="sidebar-tabs" id="sidebar-tabs" role="tablist"></div>
<nav class="sidebar-nav" id="sidebar-nav" aria-label="Documentation"><noscript><p class="sidebar-noscript"><a href="{root}index.html">Index</a></p></noscript></nav>
</aside>
<main class="content" id="content">
<div class="topbar">
<form class="search" action="{root}search.html" method="get" autocomplete="off" role="search">
<input type="search" name="q" id="search-input" placeholder="Press / to search" aria-label="Search">
<ul class="search-suggestions" id="search-suggestions" hidden></ul>
</form>
<button class="theme-toggle" type="button" aria-label="Toggle night mode"><span class="icon-moon">{moon}</span><span class="icon-sun">{sun}</span></button>
</div>
<div class="content-inner" id="content-inner">
{body}
<footer class="footer">
<p>Built with <a href="https://github.com/Meow/fdoc">fdoc</a>.</p>
</footer>
</div>
</main>
<script src="{root}assets/sidebar_items.js"></script>
<script src="{root}assets/app.js"></script>
</body>
</html>
"#,
        title = esc(title),
        label = esc(&project_label(project)),
        name = esc(&project.title),
        version = project.version.as_ref().map(|v| format!(" <span class=\"sidebar-project-version\">v{}</span>", esc(v))).unwrap_or_default(),
        group = esc(active.group_dir.unwrap_or("")),
        module = esc(active.module_slug.unwrap_or("")),
        menu = MENU_ICON,
        moon = MOON_ICON,
        sun = SUN_ICON,
    );
    out
}

/// The navigation tree shared by every page: sections, groups and modules.
fn sidebar_items(project: &Project) -> String {
    let mut sections = Vec::new();
    for section in &project.sections {
        let mut groups = Vec::new();
        for group in &section.groups {
            let modules: Vec<String> = group
                .modules
                .iter()
                .map(|m| format!("{{\"t\":{},\"s\":{}}}", json_str(&m.title), json_str(&m.slug)))
                .collect();
            groups.push(format!(
                "{{\"t\":{},\"d\":{},\"core\":{},\"m\":[{}]}}",
                json_str(&group.title),
                json_str(&group.dir),
                group.is_core,
                modules.join(",")
            ));
        }
        sections.push(format!("{{\"t\":{},\"g\":[{}]}}", json_str(&section.title), groups.join(",\n")));
    }
    format!("window.sidebarItems = [\n{}\n];\n", sections.join(",\n"))
}

/// The function list of the active module, moved under its sidebar entry by the script.
fn module_functions_template(m: &Module) -> String {
    if m.functions.is_empty() {
        return String::new();
    }
    let mut out = String::from("<template id=\"module-functions\"><ul class=\"sidebar-functions\">");
    for (cat, fns) in m.grouped() {
        let _ = write!(out, "<li class=\"sidebar-category\" data-anchor=\"{}\">{}</li>", esc(&category_anchor(&cat.name)), esc(&cat.name));
        for f in fns {
            let _ = write!(out, "<li data-realm=\"{}\"><a href=\"#{}\">{}</a></li>", realm_parts(f.realm).0, f.anchor, esc(&f.name));
        }
    }
    out.push_str("</ul></template>\n");
    out
}

fn category_anchor(name: &str) -> String {
    crate::model::slugify(name).to_ascii_lowercase()
}

fn resolver<'a>(project: &'a Project, root: &'a str) -> impl Fn(&str) -> Option<String> + 'a {
    move |r| project.resolve(r).map(|u| format!("{root}{u}"))
}

fn md(project: &Project, root: &str, text: &str) -> String {
    let resolve = resolver(project, root);
    markdown::render(text, &resolve)
}

fn md_inline(project: &Project, root: &str, text: &str) -> String {
    let resolve = resolver(project, root);
    markdown::inline(text, &resolve)
}

fn reference_link(project: &Project, root: &str, reference: &str) -> String {
    match project.resolve(reference) {
        Some(url) => format!("<a href=\"{root}{}\"><code>{}</code></a>", esc(&url), esc(reference)),
        None => format!("<code>{}</code>", esc(reference)),
    }
}

fn source_link(project: &Project, file: &str, line: usize) -> String {
    match &project.source_url {
        Some(base) => format!("<a class=\"source-link\" href=\"{}{}#L{line}\" title=\"View source\">{}:{line}</a>", esc(base), esc(file), esc(file)),
        None => format!("<span class=\"source-link\">{}:{line}</span>", esc(file)),
    }
}

/// The class suffix, label and tooltip of a realm.
fn realm_parts(realm: Realm) -> (&'static str, &'static str) {
    match realm {
        Realm::Server => ("server", "Server-side"),
        Realm::Client => ("client", "Client-side"),
        Realm::Shared => ("shared", "Shared (server and client)"),
    }
}

/// The colour-coded realm marker: orange for the client, blue for the
/// server, split for shared code.
fn realm_pill(realm: Realm) -> String {
    let (name, title) = realm_parts(realm);
    format!("<span class=\"realm realm-{name}\" title=\"{title}\">{name}</span>")
}

/// The realm every function of a module runs in, when it is one side only.
fn module_realm(m: &Module) -> Option<Realm> {
    let first = m.functions.first()?.realm;
    (first != Realm::Shared && m.functions.iter().all(|f| f.realm == first)).then_some(first)
}

/// Splits a Markdown text into its first paragraph and the rest.
fn split_first_paragraph(text: &str) -> (String, String) {
    let lines: Vec<&str> = text.trim().lines().collect();
    match lines.iter().position(|l| l.trim().is_empty()) {
        Some(p) => (lines[..p].join("\n"), lines[p..].join("\n").trim().to_string()),
        None => (lines.join("\n"), String::new()),
    }
}

fn type_html(ty: &Option<String>) -> String {
    match ty {
        Some(t) => format!("<span class=\"type\">{}</span>", esc(t)),
        None => String::new(),
    }
}

fn project_index(project: &Project) -> String {
    let path = "index.html";
    let root = root_for(path);
    let mut body = String::new();
    let _ = write!(body, "<h1>{}", esc(&project.title));
    if let Some(v) = &project.version {
        let _ = write!(body, " <small class=\"version\">v{}</small>", esc(v));
    }
    body.push_str("</h1>\n");
    if let Some(s) = &project.summary {
        let _ = writeln!(body, "<p class=\"lead\">{}</p>", md_inline(project, &root, s));
    }
    if let Some(d) = project.description.as_deref().filter(|d| project.summary.as_deref() != Some(d)) {
        body.push_str(&md(project, &root, d));
    }

    let total_modules: usize = project.all_modules().count();
    let total_functions: usize = project.all_modules().map(|(_, m)| m.functions.len()).sum();
    let documented: usize = project.all_modules().map(|(_, m)| m.functions.iter().filter(|f| f.doc.is_some()).count()).sum();
    let _ = writeln!(body, "<p class=\"stats\">{total_modules} modules · {total_functions} functions · {documented} documented</p>");

    for section in &project.sections {
        let _ = writeln!(body, "<section class=\"overview-section\"><h2>{}</h2>", esc(&section.title));
        for group in &section.groups {
            if group.is_core {
                body.push_str(&module_list(project, &root, group));
            } else {
                let _ = writeln!(body, "<div class=\"summary-row\"><div class=\"summary-signature\"><a href=\"{dir}/index.html\">{title}</a> <span class=\"muted\">{n} modules</span></div><div class=\"summary-synopsis\">{desc}</div></div>",
                    dir = group.dir,
                    title = esc(&group.title),
                    n = group.modules.len(),
                    desc = group.description.as_deref().map(|d| md_inline(project, &root, &summary_of(d))).unwrap_or_default(),
                );
            }
        }
        body.push_str("</section>\n");
    }
    layout(project, path, &project.title, Active::default(), &body)
}

fn module_list(project: &Project, root: &str, group: &Group) -> String {
    let mut out = String::from("<div class=\"module-list\">\n");
    for m in &group.modules {
        let _ = writeln!(out, "<div class=\"summary-row\"><div class=\"summary-signature\"><a href=\"{root}{dir}/{slug}.html\">{title}</a> <span class=\"badge badge-{badge}\">{badge}</span>{env}</div><div class=\"summary-synopsis\">{desc}</div></div>",
            dir = group.dir,
            slug = m.slug,
            title = esc(&m.title),
            badge = m.kind.badge(),
            env = m.environment().map(environment_flag).unwrap_or_default(),
            desc = md_inline(project, root, &m.summary()),
        );
    }
    out.push_str("</div>\n");
    out
}

fn group_index(project: &Project, group: &Group) -> String {
    let path = format!("{}/index.html", group.dir);
    let root = root_for(&path);
    let mut body = String::new();
    let _ = write!(body, "<h1>{}", esc(&group.title));
    if let Some(v) = &group.version {
        let _ = write!(body, " <small class=\"version\">v{}</small>", esc(v));
    }
    body.push_str("</h1>\n");
    if let Some(d) = &group.description {
        let (lead, rest) = split_first_paragraph(d);
        let _ = writeln!(body, "<p class=\"lead\">{}</p>", md_inline(project, &root, &lead));
        if !rest.is_empty() {
            body.push_str(&md(project, &root, &rest));
        }
    }
    if let Some(a) = &group.author {
        let _ = writeln!(body, "<p class=\"muted\">By {}</p>", esc(a));
    }
    body.push_str("<h2>Modules</h2>\n");
    body.push_str(&module_list(project, &root, group));
    layout(project, &path, &group.title, Active { group_dir: Some(&group.dir), module_slug: None }, &body)
}

fn module_page(project: &Project, group: &Group, m: &Module) -> String {
    let path = format!("{}/{}.html", group.dir, m.slug);
    let root = root_for(&path);
    let mut body = String::new();

    let _ = write!(
        body,
        "<div class=\"page-header\"><h1>{} <span class=\"badge badge-{badge}\">{badge}</span>{realm}{env}</h1>",
        esc(&m.title),
        badge = m.kind.badge(),
        realm = module_realm(m).map(realm_pill).unwrap_or_default(),
        env = m.environment().map(environment_flag).unwrap_or_default(),
    );
    let (subtitle, lead) = if m.kind == ModuleKind::Hooks { (None, m.subtitle.as_ref()) } else { (m.subtitle.as_ref(), None) };
    if let Some(sub) = subtitle {
        let _ = write!(body, "<p class=\"subtitle\">{}</p>", esc(sub));
    }
    if let ModuleKind::Class { extends: Some(base) } = &m.kind {
        let _ = write!(body, "<p class=\"subtitle\">Extends {}</p>", reference_link(project, &root, base));
    }
    if !group.is_core {
        let _ = write!(body, "<p class=\"subtitle\">Part of <a href=\"{root}{}/index.html\">{}</a></p>", group.dir, esc(&group.title));
    }
    body.push_str("</div>\n");
    if let Some(lead) = lead {
        let _ = writeln!(body, "<p class=\"lead\">{}</p>", esc(lead));
    }

    if let Some(doc) = &m.doc {
        body.push_str("<section class=\"moduledoc\">\n");
        body.push_str(&doc_body(project, &root, doc, None));
        body.push_str("</section>\n");
    }

    let grouped = m.grouped();
    if !grouped.is_empty() {
        body.push_str("<section class=\"summary\"><h2 id=\"summary\">Summary</h2>\n");
        for (cat, fns) in &grouped {
            let _ = writeln!(body, "<div class=\"summary-group\"><h3>{}</h3>", esc(&cat.name));
            for f in fns {
                let _ = writeln!(body, "<div class=\"summary-row\"><div class=\"summary-signature\"><a href=\"#{anchor}\">{sig}</a>{realm}{flags}</div><div class=\"summary-synopsis\">{desc}</div></div>",
                    anchor = f.anchor,
                    sig = esc(&f.signature()),
                    realm = realm_pill(f.realm),
                    flags = flags(f),
                    desc = md_inline(project, &root, &f.summary()),
                );
            }
            body.push_str("</div>\n");
        }
        body.push_str("</section>\n");

        for (cat, fns) in &grouped {
            let _ = writeln!(body, "<section class=\"details-group\"><h2 id=\"{}\">{}</h2>", esc(&category_anchor(&cat.name)), esc(&cat.name));
            if !cat.description.is_empty() {
                body.push_str(&md(project, &root, &cat.description));
            }
            for f in fns {
                body.push_str(&function_detail(project, &root, f));
            }
            body.push_str("</section>\n");
        }
    }

    if !m.files.is_empty() {
        body.push_str("<section class=\"sources\"><h2>Defined in</h2><ul>");
        for file in &m.files {
            let _ = write!(body, "<li>{}</li>", source_link(project, file, 1).replace(":1<", "<").replace("#L1\"", "\""));
        }
        body.push_str("</ul></section>\n");
    }

    body.push_str(&module_functions_template(m));
    layout(project, &path, &m.title, Active { group_dir: Some(&group.dir), module_slug: Some(&m.slug) }, &body)
}

/// The pill of an `@environment` tag, such as `development`.
fn environment_flag(env: &Environment) -> String {
    format!(" <span class=\"flag flag-environment\" title=\"{}\">{}</span>", esc(&env.title()), esc(&env.name))
}

fn flags(f: &Function) -> String {
    let mut out = String::new();
    if let Some(doc) = &f.doc {
        if let Some(env) = &doc.environment {
            out.push_str(&environment_flag(env));
        }
        if doc.is_deprecated() {
            out.push_str(" <span class=\"flag flag-deprecated\">deprecated</span>");
        }
        if doc.is_internal() {
            out.push_str(" <span class=\"flag flag-internal\">internal</span>");
        }
    }
    out
}

fn function_detail(project: &Project, root: &str, f: &Function) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "<section class=\"detail\" id=\"{anchor}\">\n<div class=\"detail-header\">\n<h3 class=\"signature\"><span class=\"owner\">{owner}</span>{name}<span class=\"params\">({params})</span></h3> {realm}{flags}\n<a href=\"#{anchor}\" class=\"detail-link\" title=\"Link to this function\">{icon}</a>\n{source}\n</div>\n<section class=\"docstring\">",
        anchor = f.anchor,
        owner = match f.sep {
            Sep::None => String::new(),
            Sep::Dot => format!("{}.", esc(&f.owner)),
            Sep::Colon => format!("{}:", esc(&f.owner)),
        },
        name = esc(&f.name),
        params = esc(&f.params.join(", ")),
        realm = realm_pill(f.realm),
        flags = flags(f),
        icon = LINK_ICON,
        source = source_link(project, &f.file, f.line),
    );
    out.push_str(&hook_info(project, root, f));
    match (&f.doc, &f.client_doc) {
        (doc, Some(client)) => {
            let (realm, label) = match f.doc_realm {
                Realm::Shared => ("shared", "Shared"),
                _ => ("server", "On the server"),
            };
            let _ = writeln!(out, "<div class=\"realm-doc realm-doc-{realm}\"><p class=\"realm-doc-title\">{label}</p>");
            match doc {
                Some(doc) => out.push_str(&doc_body(project, root, doc, Some(f))),
                None => out.push_str("<p class=\"muted\">No documentation available.</p>\n"),
            }
            out.push_str("</div>\n<div class=\"realm-doc realm-doc-client\"><p class=\"realm-doc-title\">On the client</p>\n");
            out.push_str(&doc_body(project, root, client, Some(f)));
            out.push_str("</div>\n");
        }
        (Some(doc), None) => out.push_str(&doc_body(project, root, doc, Some(f))),
        (None, None) => {
            out.push_str("<p class=\"muted\">No documentation available.</p>\n");
            if f.kind == FunctionKind::Hook {
                out.push_str(&arguments_list(f));
            }
        }
    }
    out.push_str(&otherwise_block(project, root, f));
    if f.kind == FunctionKind::Hook {
        if !f.callers.is_empty() {
            out.push_str("<h4>Called from</h4>\n<ul class=\"callers\">");
            for caller in &f.callers {
                let _ = write!(out, "<li>{}</li>", reference_link(project, root, caller));
            }
            out.push_str("</ul>\n");
        }
        out.push_str(&sources_list("Call sites", project, f));
    } else if f.sources.len() > 1 {
        out.push_str(&sources_list("Defined in", project, f));
    }
    out.push_str("</section>\n</section>\n");
    out
}

/// What the `else` arm definition of a branch twin does: its description,
/// without the first paragraph when that is the primary definition's, and
/// its parameters when they differ from the primary definition's.
/// Under `@environment`, the admonition already names the condition.
fn otherwise_block(project: &Project, root: &str, f: &Function) -> String {
    let Some(other) = &f.otherwise else { return String::new() };
    let environment = f.doc.as_ref().is_some_and(|d| d.environment.is_some());
    let title = if environment { "In other environments" } else { "Otherwise" };
    let mut out = format!("<div class=\"otherwise\"><p class=\"otherwise-title\">{title} {}</p>\n", source_link(project, &other.file, other.line));
    if !environment {
        let _ = writeln!(out, "<p>When <code>{}</code> does not hold:</p>", esc(&other.condition));
    }
    match &other.doc {
        Some(doc) => {
            let primary = f.doc.as_ref();
            let (lead, rest) = split_first_paragraph(&doc.description);
            let description = match primary.map(|p| &p.description) {
                Some(d) if *d == doc.description => String::new(),
                Some(d) if split_first_paragraph(d).0 == lead => rest,
                _ => doc.description.clone(),
            };
            if !description.is_empty() {
                out.push_str(&md(project, root, &description));
            }
            if !doc.params.is_empty() && primary.is_none_or(|p| p.params != doc.params) {
                out.push_str("<h4>Parameters</h4>\n");
                out.push_str(&params_list(project, root, &doc.params));
            }
        }
        None => out.push_str("<p class=\"muted\">No documentation available.</p>\n"),
    }
    out.push_str("</div>\n");
    out
}

/// The line under a hook handler's header: the hook it implements and the
/// `hook.Add` identifier.
fn hook_info(project: &Project, root: &str, f: &Function) -> String {
    let mut parts = Vec::new();
    if let Some(key) = &f.implements {
        let name = key.strip_prefix("hook:").unwrap_or(key);
        let link = match project.resolve(key) {
            Some(url) => format!("<a href=\"{root}{}\"><code>{}</code></a>", esc(&url), esc(name)),
            None => format!("<code>{}</code>", esc(name)),
        };
        parts.push(format!("Implements hook {link}"));
    }
    if f.kind == FunctionKind::HookAdd {
        parts.push(match &f.hook_id {
            Some(id) => format!("<span class=\"muted\"><code>hook.Add</code> id: <code>{}</code></span>", esc(id)),
            None => "<span class=\"muted\">Added with <code>hook.Add</code></span>".to_string(),
        });
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("<p class=\"hook-info\">{}</p>\n", parts.join("<span class=\"sep\"> · </span>"))
}

/// The definitions or call sites of a function, each with its realm.
fn sources_list(title: &str, project: &Project, f: &Function) -> String {
    let mut out = format!("<h4>{title}</h4>\n<ul class=\"source-list\">");
    for (file, line, realm) in &f.sources {
        let _ = write!(out, "<li>{} {}</li>", source_link(project, file, *line), realm_pill(*realm));
    }
    out.push_str("</ul>\n");
    out
}

/// The arguments a hook is called with. They are expressions at the call
/// site rather than parameter names, so they are shown as code.
fn arguments_list(f: &Function) -> String {
    if f.params.is_empty() {
        return String::new();
    }
    let mut out = String::from("<h4>Arguments</h4>\n<ul class=\"params arguments\">");
    for p in &f.params {
        let _ = write!(out, "<li><code>{}</code></li>", esc(p));
    }
    out.push_str("</ul>\n");
    out
}

fn returns_list(project: &Project, root: &str, returns: &[Return]) -> String {
    let mut out = String::from("<ul class=\"returns\">");
    for r in returns {
        let _ = write!(out, "<li>{}", type_html(&r.ty));
        if !r.description.is_empty() {
            let _ = write!(out, " <span class=\"desc\">{}</span>", md_inline(project, root, &r.description));
        }
        out.push_str("</li>");
    }
    out.push_str("</ul>\n");
    out
}

/// Renders a doc block: admonitions, description, variants, parameters,
/// returns, aliases and references.
fn doc_body(project: &Project, root: &str, doc: &DocBlock, f: Option<&Function>) -> String {
    let mut out = String::new();

    if let Some(env) = &doc.environment {
        let _ = writeln!(out, "<div class=\"admonition environment\"><p class=\"admonition-title\">{}</p><p>{}</p></div>", esc(&env.title()), md_inline(project, root, &env.text()));
    }
    if doc.is_deprecated() {
        out.push_str("<div class=\"admonition deprecated\"><p class=\"admonition-title\">Deprecated");
        if let Some(v) = &doc.deprecation_version {
            let _ = write!(out, " since {}", esc(v));
        }
        out.push_str("</p>");
        if let Some(reason) = &doc.deprecation {
            let _ = write!(out, "<p>{}</p>", md_inline(project, root, reason));
        }
        out.push_str("</div>\n");
    }
    for w in &doc.warnings {
        let internal = w.label.as_deref().is_some_and(|l| l.eq_ignore_ascii_case("internal"));
        let title = w.label.clone().unwrap_or_else(|| "Warning".to_string());
        let _ = write!(out, "<div class=\"admonition {}\"><p class=\"admonition-title\">{}</p>", if internal { "internal" } else { "warning" }, esc(&title));
        if internal {
            out.push_str("<p>This function is internal. It is not part of the public API and may change without notice.</p>");
        }
        if !w.text.is_empty() {
            let _ = write!(out, "<p>{}</p>", md_inline(project, root, &w.text));
        }
        out.push_str("</div>\n");
    }

    out.push_str(&md(project, root, &doc.description));

    if !doc.variants.is_empty() {
        out.push_str("<h4>Variants</h4>\n<ul class=\"variants\">\n");
        for v in &doc.variants {
            let signature = f.map(|f| display_variant(&v.signature, f)).unwrap_or_else(|| v.signature.clone());
            let _ = write!(out, "<li><code class=\"variant-signature\">{}</code>", esc(&signature));
            if !v.description.is_empty() {
                let _ = write!(out, "<p>{}</p>", md_inline(project, root, &v.description));
            }
            if !v.params.is_empty() {
                out.push_str(&params_list(project, root, &v.params));
            }
            if !v.returns.is_empty() {
                out.push_str("<h5>Returns</h5>\n");
                out.push_str(&returns_list(project, root, &v.returns));
            }
            out.push_str("</li>\n");
        }
        out.push_str("</ul>\n");
    }

    if !doc.params.is_empty() {
        out.push_str("<h4>Parameters</h4>\n");
        out.push_str(&params_list(project, root, &doc.params));
    } else if let Some(f) = f.filter(|f| f.kind == FunctionKind::Hook) {
        out.push_str(&arguments_list(f));
    } else if let Some(f) = f {
        // Undocumented parameters are still listed from the signature.
        if !f.params.is_empty() && doc.variants.is_empty() {
            out.push_str("<h4>Parameters</h4>\n<ul class=\"params\">");
            for p in &f.params {
                let _ = write!(out, "<li><span class=\"param-name\">{}</span></li>", esc(p));
            }
            out.push_str("</ul>\n");
        }
    }

    if !doc.returns.is_empty() {
        out.push_str("<h4>Returns</h4>\n");
        out.push_str(&returns_list(project, root, &doc.returns));
    }

    if !doc.aliases.is_empty() {
        out.push_str("<h4>Aliases</h4>\n<ul class=\"aliases\">");
        for a in &doc.aliases {
            let _ = write!(out, "<li><code>{}</code></li>", esc(a));
        }
        out.push_str("</ul>\n");
    }

    if !doc.see.is_empty() {
        out.push_str("<h4>See also</h4>\n<ul class=\"see-also\">");
        for s in &doc.see {
            let _ = write!(out, "<li>{}</li>", reference_link(project, root, s));
        }
        out.push_str("</ul>\n");
    }

    for (name, text) in &doc.other {
        let _ = writeln!(out, "<p class=\"tag\"><strong>{}</strong> {}</p>", esc(name), md_inline(project, root, text));
    }

    out
}

/// Shows a variant signature with the resolved owner (`Player:find_item(id)`
/// rather than `player_meta:find_item(id)`).
fn display_variant(signature: &str, f: &Function) -> String {
    let needle = format!("{}(", f.name);
    match signature.find(&needle) {
        Some(p) if p > 0 && signature[..p].ends_with([':', '.']) => {
            let sep = match f.sep {
                Sep::Colon => ":",
                _ => ".",
            };
            format!("{}{sep}{}", f.owner, &signature[p..])
        }
        _ => signature.to_string(),
    }
}

fn params_list(project: &Project, root: &str, params: &[crate::doc::Param]) -> String {
    let mut out = String::from("<ul class=\"params\">");
    for p in params {
        let _ = write!(out, "<li><span class=\"param-name\">{}</span>", esc(&p.name));
        if p.ty.is_some() {
            let _ = write!(out, " {}", type_html(&p.ty));
        }
        if let Some(d) = &p.default {
            let _ = write!(out, " <span class=\"default\">optional, defaults to <code>{}</code></span>", esc(d));
        }
        if !p.description.is_empty() {
            let _ = write!(out, " <span class=\"desc\">{}</span>", md_inline(project, root, &p.description));
        }
        out.push_str("</li>");
    }
    out.push_str("</ul>\n");
    out
}

fn search_page(project: &Project) -> String {
    let body = "<h1>Search</h1>\n<p class=\"muted\" id=\"search-status\">Type in the search box to find modules and functions.</p>\n<div id=\"search-results\" class=\"search-results\"></div>\n";
    layout(project, "search.html", "Search", Active::default(), body)
}

/// The search index: one entry per group, module and function, with its
/// title (`t`), kind (`k`), URL (`r`), synopsis (`d`), parent (`g`) and,
/// for functions, realm (`rl`).
fn search_data(project: &Project) -> String {
    let mut items: Vec<String> = Vec::new();
    let entry = |title: &str, kind: &str, url: &str, desc: &str, group: &str, realm: Option<Realm>| {
        let desc = truncate(desc, 140);
        let realm = realm.map(|r| format!(",\"rl\":\"{}\"", realm_parts(r).0)).unwrap_or_default();
        format!("{{\"t\":{},\"k\":{},\"r\":{},\"d\":{},\"g\":{}{realm}}}", json_str(title), json_str(kind), json_str(url), json_str(&desc), json_str(group))
    };
    for section in &project.sections {
        for group in &section.groups {
            if !group.is_core {
                items.push(entry(&group.title, "group", &format!("{}/index.html", group.dir), &summary_of(group.description.as_deref().unwrap_or("")), &section.title, None));
            }
            for m in &group.modules {
                let page = format!("{}/{}.html", group.dir, m.slug);
                items.push(entry(&m.title, m.kind.badge(), &page, &m.summary(), &group.title, None));
                for f in &m.functions {
                    let kind = match f.kind {
                        FunctionKind::Declared => "function",
                        FunctionKind::Hook => "hook",
                        FunctionKind::HookAdd => "handler",
                    };
                    items.push(entry(&f.qualified_signature(), kind, &format!("{page}#{}", f.anchor), &f.summary(), &m.title, Some(f.realm)));
                }
            }
        }
    }
    format!("window.searchData = [\n{}\n];\n", items.join(",\n"))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    let cut = cut.rsplit_once(' ').map(|(head, _)| head.to_string()).unwrap_or(cut);
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Layout;
    use crate::model::{build, placed, BuildOptions};
    use std::collections::HashMap;
    use crate::scanner::scan;

    #[test]
    fn renders_pages() {
        let files = vec![
            (
                "lib/a.lua".to_string(),
                scan("--- Class A.\nclass 'A'\n--- Does x.\n-- @param n=1 [Number count]\n-- @return [Boolean ok]\n-- @variant A:do_x(n, m)\n--   @param m [String mode]\n--   @return [String text]\n-- @see [A#other]\nfunction A:do_x(n)\nend\nfunction A:other() end"),
            ),
            ("lib/sv_a.lua".to_string(), scan("function A:on_server()\n  hook.Run('Spawned', self, 1)\nend")),
            ("lib/sv_k.lua".to_string(), scan("--- Server twin.\nfunction A:twin() end")),
            ("lib/cl_k.lua".to_string(), scan("--- Client twin.\nfunction A:twin() end\nhook.Run('Spawned', LocalPlayer())")),
            ("lib/sh_gm.lua".to_string(), scan("--- Handles spawns.\nfunction GM:Spawned(obj, n) end")),
            ("lib/cl_add.lua".to_string(), scan("hook.Add('Spawned', 'my_id', function(obj) end)")),
        ];
        let layout = Layout::default_for("Test");
        let meta = HashMap::new();
        let project = build(placed(&layout, files), BuildOptions { title: Some("Test"), source_url: Some("https://example.com/"), ..BuildOptions::new(&layout, &meta) });
        let pages = render_all(&project);
        let page = |path: &str| pages.iter().find(|p| p.path == path).map(|p| p.content.as_str()).unwrap_or_else(|| panic!("no page {path}"));

        let a = page("flux/A.html");
        assert!(a.contains("<section class=\"detail\" id=\"do_x\">"));
        assert!(a.contains("<a href=\"../flux/A.html#other\"><code>A#other</code></a>"));
        assert!(a.contains("defaults to <code>1</code>"));
        assert!(a.contains("https://example.com/lib/a.lua#L10"));
        assert!(a.contains("No documentation available."));
        assert!(a.contains("data-group=\"flux\" data-module=\"A\""));
        assert!(a.contains("<template id=\"module-functions\">"));
        assert!(a.contains("<li data-realm=\"server\"><a href=\"#on_server\">on_server</a></li>"), "{a}");
        assert!(a.contains("on_server()</a><span class=\"realm realm-server\" title=\"Server-side\">server</span>"), "{a}");
        assert!(a.contains("<span class=\"realm realm-shared\" title=\"Shared (server and client)\">shared</span>"));
        assert!(!a.contains("flag-realm"));
        assert!(!a.contains("<h1>A <span class=\"badge badge-class\">class</span><span class=\"realm"), "mixed realms are not marked in the header");

        // Variant returns sit under the variant's parameters, the others below.
        let variant = &a[a.find("<ul class=\"variants\">").unwrap()..];
        assert!(variant.starts_with("<ul class=\"variants\">\n<li><code class=\"variant-signature\">A:do_x(n, m)</code><ul class=\"params\"><li><span class=\"param-name\">m</span>"), "{variant}");
        assert!(variant.contains("<h5>Returns</h5>\n<ul class=\"returns\"><li><span class=\"type\">String</span> <span class=\"desc\">text</span></li></ul>\n</li>"), "{variant}");
        assert!(variant.contains("<h4>Returns</h4>\n<ul class=\"returns\"><li><span class=\"type\">Boolean</span> <span class=\"desc\">ok</span></li></ul>"), "{variant}");

        // Merged realm twins: both docs and both definitions.
        let twin = &a[a.find("id=\"twin\"").unwrap()..];
        assert!(twin.contains("<div class=\"realm-doc realm-doc-server\"><p class=\"realm-doc-title\">On the server</p>\n<p>Server twin.</p>"), "{twin}");
        assert!(twin.contains("<p class=\"realm-doc-title\">On the client</p>\n<p>Client twin.</p>"), "{twin}");
        assert!(twin.contains("<h4>Defined in</h4>\n<ul class=\"source-list\"><li><a class=\"source-link\" href=\"https://example.com/lib/sv_k.lua#L2\" title=\"View source\">lib/sv_k.lua:2</a> <span class=\"realm realm-server\""), "{twin}");
        assert!(twin.contains("lib/cl_k.lua:2</a> <span class=\"realm realm-client\""), "{twin}");

        // The hooks page: arguments as code, callers as references, call sites.
        let hooks = page("flux/Hooks.html");
        assert!(hooks.contains("<h1>Hooks <span class=\"badge badge-hooks\">hooks</span>"));
        assert!(hooks.contains("<p class=\"lead\">Hooks called by Test</p>"), "{hooks}");
        assert!(hooks.contains("Spawned(self, 1)</a><span class=\"realm realm-shared\""), "{hooks}");
        assert!(hooks.contains("<h4>Arguments</h4>\n<ul class=\"params arguments\"><li><code>self</code></li><li><code>1</code></li></ul>"), "{hooks}");
        assert!(hooks.contains("<h4>Called from</h4>\n<ul class=\"callers\"><li><a href=\"../flux/A.html#on_server\"><code>A:on_server</code></a></li></ul>"), "{hooks}");
        assert!(hooks.contains("<h4>Call sites</h4>"));
        let sites = &hooks[hooks.find("<h4>Call sites</h4>").unwrap()..];
        assert!(sites.contains("lib/sv_a.lua:2</a> <span class=\"realm realm-server\""), "{sites}");
        assert!(sites.contains("lib/cl_k.lua:3</a> <span class=\"realm realm-client\""), "{sites}");

        // Handlers link to the hook they implement.
        let gm = page("flux/GM.html");
        assert!(gm.contains("<p class=\"hook-info\">Implements hook <a href=\"../flux/Hooks.html#Spawned\"><code>Spawned</code></a></p>"), "{gm}");
        let added = page("flux/hook.Add.html");
        assert!(added.contains("Implements hook <a href=\"../flux/Hooks.html#Spawned\"><code>Spawned</code></a><span class=\"sep\"> · </span><span class=\"muted\"><code>hook.Add</code> id: <code>my_id</code></span>"), "{added}");
        assert!(added.contains("<h1>hook.Add <span class=\"badge badge-handlers\">handlers</span><span class=\"realm realm-client\""), "a one-realm module is marked in its header");

        let items = page("assets/sidebar_items.js");
        assert!(items.contains("{\"t\":\"A\",\"s\":\"A\"}"), "{items}");
        let data = page("assets/search_data.js");
        assert!(data.contains("\"t\":\"A:do_x(n)\""));
        assert!(data.contains("\"t\":\"A:on_server()\",\"k\":\"function\",\"r\":\"flux/A.html#on_server\",\"d\":\"\",\"g\":\"A\",\"rl\":\"server\"}"), "{data}");
        assert!(data.contains("\"t\":\"Spawned(self, 1)\",\"k\":\"hook\""), "{data}");
    }

    #[test]
    fn renders_environments_and_branch_twins() {
        let files = vec![
            (
                "lib/sh_debug.lua".to_string(),
                scan(
                    "if is_development then
  --- Prints a metric.
  -- @environment [development]
  -- @param id [String metric name]
  -- @param format='{id}' [String message]
  function print_metric(id, format) end
  --- Adds a metric.
  -- @param id [String metric name]
  function add_metric(id) end
else
  --- Does nothing, since metrics are only collected in development.
  -- @param id [String metric name]
  -- @param format=nil [String message]
  function print_metric(id, format) end
  --- Adds a metric.
  -- @param id [String metric name]
  function add_metric(id) end
end
if is_development then
  --- Resets the metrics.
  --
  -- Clears every list.
  function reset() end
else
  --- Resets the metrics.
  --
  -- Does nothing here.
  function reset() end
end",
                ),
            ),
            ("lib/sh_seed.lua".to_string(), scan("--- Seeds the database.\n-- @env [production] Skipped when `SEED` is set.\n\nfunction Seed.run() end")),
        ];
        let layout = Layout::default_for("Test");
        let meta = HashMap::new();
        let project = build(placed(&layout, files), BuildOptions { title: Some("Test"), ..BuildOptions::new(&layout, &meta) });
        let pages = render_all(&project);
        let page = |path: &str| pages.iter().find(|p| p.path == path).map(|p| p.content.as_str()).unwrap_or_else(|| panic!("no page {path}"));

        let globals = page("flux/Globals.html");
        assert_eq!(globals.matches("<section class=\"detail\"").count(), 3, "{globals}");
        let pill = "<span class=\"flag flag-environment\" title=\"Development only\">development</span>";
        assert!(globals.contains(&format!("print_metric(id, format)</a><span class=\"realm realm-shared\" title=\"Shared (server and client)\">shared</span> {pill}")), "{globals}");
        let detail = &globals[globals.find("id=\"print_metric\"").unwrap()..];
        assert!(detail.contains(&format!("shared</span> {pill}\n<a href=\"#print_metric\"")), "{detail}");
        assert!(detail.contains("<section class=\"docstring\">\n<div class=\"admonition environment\"><p class=\"admonition-title\">Development only</p><p>Only available in the development environment.</p></div>\n<p>Prints a metric.</p>"), "{detail}");
        let other = &detail[detail.find("<div class=\"otherwise\">").unwrap()..];
        assert!(other.starts_with("<div class=\"otherwise\"><p class=\"otherwise-title\">In other environments <span class=\"source-link\">lib/sh_debug.lua:14</span></p>\n<p>Does nothing, since metrics are only collected in development.</p>\n<h4>Parameters</h4>"), "{other}");
        assert!(!other[..other.find("</div>").unwrap()].contains("does not hold"), "{other}");
        assert!(other.contains("<h4>Defined in</h4>\n<ul class=\"source-list\"><li><span class=\"source-link\">lib/sh_debug.lua:6</span> <span class=\"realm realm-shared\""), "{other}");
        assert!(other.contains("<li><span class=\"source-link\">lib/sh_debug.lua:14</span> <span class=\"realm realm-shared\""), "{other}");

        let add = &globals[globals.find("id=\"add_metric\"").unwrap()..];
        let other = &add[add.find("<div class=\"otherwise\">").unwrap()..];
        assert!(other.starts_with("<div class=\"otherwise\"><p class=\"otherwise-title\">Otherwise <span class=\"source-link\">lib/sh_debug.lua:17</span></p>\n<p>When <code>is_development</code> does not hold:</p>\n</div>"), "identical docs are not repeated: {other}");

        let reset = &globals[globals.find("id=\"reset\"").unwrap()..];
        let other = &reset[reset.find("<div class=\"otherwise\">").unwrap()..];
        assert!(other.contains("does not hold:</p>\n<p>Does nothing here.</p>\n</div>"), "a repeated summary paragraph is left out: {other}");

        let seed = page("flux/Seed.html");
        assert!(seed.contains("<h1>Seed <span class=\"badge badge-library\">library</span> <span class=\"flag flag-environment\" title=\"Production only\">production</span></h1>"), "{seed}");
        assert!(seed.contains("<p class=\"admonition-title\">Production only</p><p>Skipped when <code>SEED</code> is set.</p>"), "{seed}");
        let index = page("index.html");
        assert!(index.contains("Seed</a> <span class=\"badge badge-library\">library</span> <span class=\"flag flag-environment\" title=\"Production only\">production</span></div>"), "{index}");
    }
}
