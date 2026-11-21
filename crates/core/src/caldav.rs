//! Minimal CalDAV (RFC 4791 / WebDAV RFC 4918) XML request builders,
//! `<multistatus>` response parser, authentication header builder, and URL helpers.
//!
//! Transport (HTTP) stays outside `fern_core` so this module remains pure and easy to test.

use std::fmt::Write as _;

/// A single `<response>` item parsed from a WebDAV/CalDAV `207 Multi-Status` XML body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DavResource {
    pub href: String,
    pub etag: Option<String>,
    pub display_name: Option<String>,
    pub calendar_data: Option<String>,
    pub resource_types: Vec<String>,
    pub is_calendar: bool,
    pub is_collection: bool,
    pub read_only: bool,
    pub status_ok: bool,
    pub current_user_principal: Option<String>,
    pub calendar_home_set: Option<String>,
}

/// Builds a `PROPFIND` body asking for `<current-user-principal/>` (and fallback calendar properties).
pub fn propfind_discovery_body() -> &'static str {
    concat!(
        r#"<?xml version="1.0" encoding="utf-8"?>"#,
        "\n",
        r#"<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">"#,
        "\n",
        r#"  <d:prop>"#,
        "\n",
        r#"    <d:current-user-principal/>"#,
        "\n",
        r#"    <c:calendar-home-set/>"#,
        "\n",
        r#"    <d:resourcetype/>"#,
        "\n",
        r#"    <d:displayname/>"#,
        "\n",
        r#"  </d:prop>"#,
        "\n",
        r#"</d:propfind>"#,
    )
}

/// Builds a `PROPFIND` body (typically `Depth: 0`) asking for `<c:calendar-home-set/>`.
pub fn propfind_home_set_body() -> &'static str {
    concat!(
        r#"<?xml version="1.0" encoding="utf-8"?>"#,
        "\n",
        r#"<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">"#,
        "\n",
        r#"  <d:prop>"#,
        "\n",
        r#"    <c:calendar-home-set/>"#,
        "\n",
        r#"  </d:prop>"#,
        "\n",
        r#"</d:propfind>"#,
    )
}

/// Builds a `PROPFIND` body (`Depth: 1`) listing calendar collections inside a home-set.
pub fn propfind_calendars_body() -> &'static str {
    concat!(
        r#"<?xml version="1.0" encoding="utf-8"?>"#,
        "\n",
        r#"<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">"#,
        "\n",
        r#"  <d:prop>"#,
        "\n",
        r#"    <d:resourcetype/>"#,
        "\n",
        r#"    <d:displayname/>"#,
        "\n",
        r#"    <c:supported-calendar-component-set/>"#,
        "\n",
        r#"    <d:current-user-privilege-set/>"#,
        "\n",
        r#"  </d:prop>"#,
        "\n",
        r#"</d:propfind>"#,
    )
}

/// Builds a `PROPFIND` body (`Depth: 1`) listing child `.ics` hrefs and ETags as a fallback
/// when a server rejects `REPORT calendar-query`.
pub fn propfind_etags_body() -> &'static str {
    concat!(
        r#"<?xml version="1.0" encoding="utf-8"?>"#,
        "\n",
        r#"<d:propfind xmlns:d="DAV:">"#,
        "\n",
        r#"  <d:prop>"#,
        "\n",
        r#"    <d:getetag/>"#,
        "\n",
        r#"    <d:resourcetype/>"#,
        "\n",
        r#"  </d:prop>"#,
        "\n",
        r#"</d:propfind>"#,
    )
}

/// Builds a CalDAV `REPORT calendar-query` XML body fetching all `VEVENT` resources with ETags
/// and inline `<c:calendar-data/>`.
pub fn calendar_query_body() -> &'static str {
    concat!(
        r#"<?xml version="1.0" encoding="utf-8"?>"#,
        "\n",
        r#"<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">"#,
        "\n",
        r#"  <d:prop>"#,
        "\n",
        r#"    <d:getetag/>"#,
        "\n",
        r#"    <c:calendar-data/>"#,
        "\n",
        r#"  </d:prop>"#,
        "\n",
        r#"  <c:filter>"#,
        "\n",
        r#"    <c:comp-filter name="VCALENDAR">"#,
        "\n",
        r#"      <c:comp-filter name="VEVENT"/>"#,
        "\n",
        r#"    </c:comp-filter>"#,
        "\n",
        r#"  </c:filter>"#,
        "\n",
        r#"</c:calendar-query>"#,
    )
}

/// Builds a CalDAV `REPORT calendar-multiget` XML body for the given resource `href`s.
pub fn calendar_multiget_body(hrefs: &[&str]) -> String {
    let mut out = String::from(concat!(
        r#"<?xml version="1.0" encoding="utf-8"?>"#,
        "\n",
        r#"<c:calendar-multiget xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">"#,
        "\n",
        r#"  <d:prop>"#,
        "\n",
        r#"    <d:getetag/>"#,
        "\n",
        r#"    <c:calendar-data/>"#,
        "\n",
        r#"  </d:prop>"#,
        "\n",
    ));
    for href in hrefs {
        let _ = writeln!(out, "  <d:href>{}</d:href>", escape_xml(href));
    }
    out.push_str("</c:calendar-multiget>");
    out
}

/// Builds an HTTP `Authorization` header value from `username` and `password`.
///
/// Supports OAuth Bearer tokens (when `password` begins with `"Bearer "` or `username`
/// is `"bearer"`) and standard HTTP Basic auth otherwise.
pub fn auth_header(username: &str, password: &str) -> Option<String> {
    let user = username.trim();
    let pass = password;
    if user.is_empty() && pass.is_empty() {
        return None;
    }
    if let Some(token) = pass.strip_prefix("Bearer ").or_else(|| pass.strip_prefix("bearer ")) {
        let token = token.trim();
        if !token.is_empty() {
            return Some(format!("Bearer {token}"));
        }
    }
    if user.eq_ignore_ascii_case("bearer") && !pass.trim().is_empty() {
        return Some(format!("Bearer {}", pass.trim()));
    }
    let creds = format!("{user}:{pass}");
    Some(format!("Basic {}", base64_encode(creds.as_bytes())))
}

/// Resolves a WebDAV `<href>` (absolute URL, protocol-relative `//...`, root-relative `/...`,
/// query/fragment reference, or relative path with `.`/`..` segments) against `base_url`.
pub fn resolve_href(base_url: &str, href: &str) -> String {
    let href = href.trim();
    if href.is_empty() {
        return base_url.to_owned();
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_owned();
    }
    let Some(sep) = base_url.find("://") else {
        return href.to_owned();
    };
    let scheme = &base_url[..sep];
    let scheme_sep = &base_url[..sep + 3];
    let rest = &base_url[sep + 3..];
    if href.starts_with("//") {
        return format!("{scheme}:{href}");
    }
    if href.starts_with('#') {
        let before_frag = base_url.split('#').next().unwrap_or(base_url);
        return format!("{before_frag}{href}");
    }
    if href.starts_with('?') {
        let before_query = base_url.split(['?', '#']).next().unwrap_or(base_url);
        return format!("{before_query}{href}");
    }
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..auth_end];
    let base_after_auth = &rest[auth_end..];
    let suffix_idx = href.find(['?', '#']).unwrap_or(href.len());
    let (href_path, href_suffix) = href.split_at(suffix_idx);
    if href_path.starts_with('/') {
        let normalized = normalize_url_path(href_path);
        return format!("{scheme_sep}{authority}{normalized}{href_suffix}");
    }
    let base_path = base_after_auth.split(['?', '#']).next().unwrap_or("");
    let base_dir = match base_path.rfind('/') {
        Some(idx) => &base_path[..=idx],
        None => "/",
    };
    let joined = format!("{base_dir}{href_path}");
    let normalized = normalize_url_path(&joined);
    format!("{scheme_sep}{authority}{normalized}{href_suffix}")
}

fn normalize_url_path(path: &str) -> String {
    let trailing_slash = path.ends_with('/') || path.ends_with("/.") || path.ends_with("/..") || path == "." || path == "..";
    let mut stack: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    let mut out = String::from("/");
    out.push_str(&stack.join("/"));
    if trailing_slash && !out.ends_with('/') {
        out.push('/');
    }
    out
}

/// Builds a canonical resource URL for a `VEVENT` with `uid` inside `calendar_url`.
pub fn event_href(calendar_url: &str, uid: &str) -> String {
    let base = calendar_url.trim_end_matches('/');
    let mut slug = String::with_capacity(uid.len());
    for ch in uid.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '@') {
            slug.push(ch);
        } else {
            slug.push('_');
        }
    }
    if slug.is_empty() {
        slug.push_str("event");
    }
    if slug.ends_with(".ics") {
        format!("{base}/{slug}")
    } else {
        format!("{base}/{slug}.ics")
    }
}

/// Normalizes HTTP/XML ETags for comparison while keeping callers free to send the raw header value.
pub fn normalize_etag(etag: &str) -> String {
    let trimmed = etag.trim();
    trimmed.strip_prefix("W/").or_else(|| trimmed.strip_prefix("w/")).unwrap_or(trimmed).trim().to_owned()
}

/// Parses a WebDAV/CalDAV `207 Multi-Status` XML body into [`DavResource`] entries.
pub fn parse_multistatus(xml: &str) -> Vec<DavResource> {
    let mut out = Vec::new();
    for block in element_blocks(xml, "response") {
        let href = first_child_text(&block, "href").unwrap_or_default();
        let mut res = DavResource { href: decode_cdata_or_xml(href).trim().to_owned(), ..Default::default() };

        let propstats = element_blocks(&block, "propstat");
        if propstats.is_empty() {
            // Some minimal servers emit properties directly inside <response>.
            apply_prop_block(&block, &mut res, true);
            res.status_ok = true;
        } else {
            for propstat in propstats {
                let status_raw = first_child_text(&propstat, "status").unwrap_or_default();
                let status_text = decode_cdata_or_xml(status_raw);
                let ok = status_is_ok(&status_text);
                if ok {
                    res.status_ok = true;
                }
                if let Some(prop) = first_element_block(&propstat, "prop") {
                    apply_prop_block(prop, &mut res, ok);
                } else {
                    apply_prop_block(&propstat, &mut res, ok);
                }
            }
        }

        if !res.href.is_empty()
            || res.etag.is_some()
            || res.calendar_data.is_some()
            || res.display_name.is_some()
            || res.current_user_principal.is_some()
            || res.calendar_home_set.is_some()
        {
            out.push(res);
        }
    }
    out
}

fn apply_prop_block(prop_xml: &str, res: &mut DavResource, ok: bool) {
    if !ok {
        return;
    }
    if res.etag.is_none()
        && let Some(etag) = first_element_inner(prop_xml, "getetag")
    {
        let etag = decode_cdata_or_xml(etag).trim().to_owned();
        if !etag.is_empty() {
            res.etag = Some(etag);
        }
    }
    if res.display_name.is_none()
        && let Some(name) = first_element_inner(prop_xml, "displayname")
    {
        let name = decode_cdata_or_xml(name).trim().to_owned();
        if !name.is_empty() {
            res.display_name = Some(name);
        }
    }
    if res.calendar_data.is_none()
        && let Some(data) = first_element_inner(prop_xml, "calendar-data")
    {
        let decoded = decode_cdata_or_xml(data);
        if !decoded.trim().is_empty() {
            res.calendar_data = Some(decoded);
        }
    }
    if res.current_user_principal.is_none()
        && let Some(principal_block) = first_element_block(prop_xml, "current-user-principal")
        && let Some(href) = first_child_text(principal_block, "href")
    {
        let href = decode_cdata_or_xml(href).trim().to_owned();
        if !href.is_empty() {
            res.current_user_principal = Some(href);
        }
    }
    if res.calendar_home_set.is_none()
        && let Some(home_block) = first_element_block(prop_xml, "calendar-home-set")
        && let Some(href) = first_child_text(home_block, "href")
    {
        let href = decode_cdata_or_xml(href).trim().to_owned();
        if !href.is_empty() {
            res.calendar_home_set = Some(href);
        }
    }
    if let Some(rt_block) = first_element_block(prop_xml, "resourcetype") {
        let tags = child_tag_local_names(rt_block);
        for tag in tags {
            if tag.eq_ignore_ascii_case("calendar") {
                res.is_calendar = true;
            } else if tag.eq_ignore_ascii_case("collection") {
                res.is_collection = true;
            }
            if !res.resource_types.iter().any(|t| t == &tag) {
                res.resource_types.push(tag);
            }
        }
    }
    if let Some(priv_block) = first_element_block(prop_xml, "current-user-privilege-set") {
        let has_write = has_tag(priv_block, "write")
            || has_tag(priv_block, "write-content")
            || has_tag(priv_block, "all");
        if !has_write {
            res.read_only = true;
        }
    }
}

fn status_is_ok(status: &str) -> bool {
    let trimmed = status.trim();
    if trimmed.is_empty() {
        return true;
    }
    // e.g. "HTTP/1.1 200 OK"
    trimmed.split_whitespace().nth(1).is_some_and(|code| code.starts_with('2'))
}

fn local_tag_name(raw_tag: &str) -> &str {
    let name = raw_tag.split_whitespace().next().unwrap_or(raw_tag).trim_end_matches('/');
    match name.rfind(':') {
        Some(idx) => &name[idx + 1..],
        None => name,
    }
}

/// Finds all top-level (non-nested within itself) element blocks matching `target` local name.
fn element_blocks(xml: &str, target: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut pos = 0;
    while let Some((inner, next_pos)) = find_next_element(xml, target, pos) {
        blocks.push(inner.to_owned());
        pos = next_pos;
    }
    blocks
}

fn first_element_block<'a>(xml: &'a str, target: &str) -> Option<&'a str> {
    find_next_element(xml, target, 0).map(|(block, _)| block)
}

fn first_element_inner<'a>(xml: &'a str, target: &str) -> Option<&'a str> {
    find_next_element_inner(xml, target, 0)
}

fn first_child_text<'a>(xml: &'a str, target: &str) -> Option<&'a str> {
    first_direct_child_inner(xml, target)
}

/// If `xml[start..]` begins with an XML comment, CDATA section, or processing instruction,
/// returns `Some(Some(end_index))` (or `Some(None)` if unclosed). Returns `None` for normal tags.
fn skip_xml_special(xml: &str, start: usize) -> Option<Option<usize>> {
    let slice = &xml[start..];
    if slice.starts_with("<![CDATA[") {
        return Some(slice[9..].find("]]>").map(|end| start + 9 + end + 3));
    }
    if slice.starts_with("<!--") {
        return Some(slice[4..].find("-->").map(|end| start + 4 + end + 3));
    }
    if slice.starts_with("<?") {
        return Some(slice[2..].find("?>").map(|end| start + 2 + end + 2));
    }
    None
}

/// Finds the inner content of the first direct child element of `xml` matching `target` local name,
/// ignoring descendant elements of sibling children, comments, CDATA, and PIs.
fn first_direct_child_inner<'a>(xml: &'a str, target: &str) -> Option<&'a str> {
    let open_lt = xml.find('<')?;
    let open_end = find_tag_end(xml, open_lt + 1)?;
    if xml[open_lt + 1..open_end].trim().ends_with('/') {
        return None;
    }
    let mut depth = 0usize;
    let mut pos = open_end + 1;
    while pos < xml.len() {
        let rel = xml[pos..].find('<')?;
        let start = pos + rel;
        if let Some(after) = skip_xml_special(xml, start) {
            pos = after?;
            continue;
        }
        let end = find_tag_end(xml, start + 1)?;
        let body = xml[start + 1..end].trim();
        if body.starts_with('/') {
            if depth == 0 {
                // Reached the closing tag of the parent element.
                return None;
            }
            depth -= 1;
            pos = end + 1;
            continue;
        }
        let self_closing = body.ends_with('/');
        if depth == 0 && local_tag_name(body).eq_ignore_ascii_case(target) {
            if self_closing {
                return Some("");
            }
            // Scan until the matching closing tag for this direct child.
            let inner_start = end + 1;
            let mut inner_depth = 1usize;
            let mut scan = inner_start;
            while scan < xml.len() {
                let next_lt = xml[scan..].find('<')?;
                let tag_start = scan + next_lt;
                if let Some(after) = skip_xml_special(xml, tag_start) {
                    scan = after?;
                    continue;
                }
                let tag_end = find_tag_end(xml, tag_start + 1)?;
                let tag_body = xml[tag_start + 1..tag_end].trim();
                if tag_body.starts_with('/') {
                    inner_depth = inner_depth.saturating_sub(1);
                    if inner_depth == 0 {
                        return Some(&xml[inner_start..tag_start]);
                    }
                } else if !tag_body.ends_with('/') {
                    inner_depth += 1;
                }
                scan = tag_end + 1;
            }
            return None;
        }
        if !self_closing {
            depth += 1;
        }
        pos = end + 1;
    }
    None
}

/// Finds the full slice `<tag ...>...</tag>` for the first occurrence starting at `from`.
fn find_next_element<'a>(xml: &'a str, target: &str, from: usize) -> Option<(&'a str, usize)> {
    let mut cursor = from;
    while cursor < xml.len() {
        let rel = xml[cursor..].find('<')?;
        let start = cursor + rel;
        if xml[start..].starts_with("<![CDATA[") {
            let end_cdata = xml[start + 9..].find("]]>")?;
            cursor = start + 9 + end_cdata + 3;
            continue;
        }
        if xml[start..].starts_with("<!--") {
            let end_comment = xml[start + 4..].find("-->")?;
            cursor = start + 4 + end_comment + 3;
            continue;
        }
        if xml[start..].starts_with("<?") {
            let end_pi = xml[start + 2..].find("?>")?;
            cursor = start + 2 + end_pi + 2;
            continue;
        }
        let close_gt = find_tag_end(xml, start + 1)?;
        let raw_inside = xml[start + 1..close_gt].trim();
        if raw_inside.starts_with('/') {
            cursor = close_gt + 1;
            continue;
        }
        let self_closing = raw_inside.ends_with('/');
        let tag_local = local_tag_name(raw_inside);
        if !tag_local.eq_ignore_ascii_case(target) {
            cursor = close_gt + 1;
            continue;
        }
        if self_closing {
            return Some((&xml[start..=close_gt], close_gt + 1));
        }
        // Scan matching depth until the closing tag with local name == target.
        let mut depth = 1usize;
        let mut scan = close_gt + 1;
        while scan < xml.len() {
            let Some(next_lt) = xml[scan..].find('<') else { break };
            let tag_start = scan + next_lt;
            if xml[tag_start..].starts_with("<![CDATA[") {
                let Some(end_c) = xml[tag_start + 9..].find("]]>") else { break };
                scan = tag_start + 9 + end_c + 3;
                continue;
            }
            if xml[tag_start..].starts_with("<!--") {
                let Some(end_c) = xml[tag_start + 4..].find("-->") else { break };
                scan = tag_start + 4 + end_c + 3;
                continue;
            }
            let Some(tag_end) = find_tag_end(xml, tag_start + 1) else { break };
            let tag_body = xml[tag_start + 1..tag_end].trim();
            if let Some(closing) = tag_body.strip_prefix('/') {
                if local_tag_name(closing.trim()).eq_ignore_ascii_case(target) {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some((&xml[start..=tag_end], tag_end + 1));
                    }
                }
            } else if !tag_body.ends_with('/') && local_tag_name(tag_body).eq_ignore_ascii_case(target) {
                depth += 1;
            }
            scan = tag_end + 1;
        }
        return None;
    }
    None
}

/// Finds the inner content of `<target ...>INNER</target>` starting at `from`.
fn find_next_element_inner<'a>(xml: &'a str, target: &str, from: usize) -> Option<&'a str> {
    let (full, _) = find_next_element(xml, target, from)?;
    let open_end = find_tag_end(full, 1)?;
    if full[1..open_end].trim().ends_with('/') {
        return Some("");
    }
    let close_start = full.rfind('<')?;
    if close_start <= open_end {
        return Some("");
    }
    Some(&full[open_end + 1..close_start])
}

fn find_tag_end(xml: &str, from: usize) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (idx, ch) in xml[from..].char_indices() {
        match (quote, ch) {
            (None, '"' | '\'') => quote = Some(ch),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(from + idx),
            _ => {}
        }
    }
    None
}

fn child_tag_local_names(xml: &str) -> Vec<String> {
    let mut names = Vec::new();
    let Some(open_end) = find_tag_end(xml, 1) else {
        return names;
    };
    let mut pos = open_end + 1;
    while pos < xml.len() {
        let Some(rel) = xml[pos..].find('<') else { break };
        let start = pos + rel;
        if let Some(after) = skip_xml_special(xml, start) {
            let Some(next) = after else { break };
            pos = next;
            continue;
        }
        let Some(end) = find_tag_end(xml, start + 1) else { break };
        let inner = xml[start + 1..end].trim();
        if !inner.starts_with('/') && !inner.starts_with('!') && !inner.starts_with('?') {
            let local = local_tag_name(inner);
            if !local.is_empty() {
                names.push(local.to_owned());
            }
        }
        pos = end + 1;
    }
    names
}

fn has_tag(xml: &str, target: &str) -> bool {
    let mut pos = 0;
    while pos < xml.len() {
        let Some(rel) = xml[pos..].find('<') else { break };
        let start = pos + rel;
        if let Some(after) = skip_xml_special(xml, start) {
            let Some(next) = after else { break };
            pos = next;
            continue;
        }
        let Some(end) = find_tag_end(xml, start + 1) else { break };
        let inner = xml[start + 1..end].trim();
        if !inner.starts_with('/') && !inner.starts_with('!') && !inner.starts_with('?')
            && local_tag_name(inner).eq_ignore_ascii_case(target)
        {
            return true;
        }
        pos = end + 1;
    }
    false
}

fn decode_cdata_or_xml(raw: &str) -> String {
    let trimmed = raw.trim();
    if !trimmed.contains("<![CDATA[") && !trimmed.contains("<!--") {
        return unescape_xml(trimmed);
    }
    let mut out = String::with_capacity(trimmed.len());
    let mut pos = 0usize;
    while pos < trimmed.len() {
        let rest = &trimmed[pos..];
        let next_cdata = rest.find("<![CDATA[").map(|i| (i, true));
        let next_comment = rest.find("<!--").map(|i| (i, false));
        let next = match (next_cdata, next_comment) {
            (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let Some((rel, is_cdata)) = next else {
            out.push_str(&unescape_xml(rest));
            break;
        };
        if rel > 0 {
            out.push_str(&unescape_xml(&rest[..rel]));
        }
        let start = pos + rel;
        if is_cdata {
            let content_start = start + 9;
            if let Some(end_rel) = trimmed[content_start..].find("]]>") {
                out.push_str(&trimmed[content_start..content_start + end_rel]);
                pos = content_start + end_rel + 3;
            } else {
                out.push_str(&trimmed[content_start..]);
                break;
            }
        } else {
            let comment_start = start + 4;
            if let Some(end_rel) = trimmed[comment_start..].find("-->") {
                pos = comment_start + end_rel + 3;
            } else {
                break;
            }
        }
    }
    out
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn unescape_xml(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        if let Some(semi) = after.find(';').filter(|i| *i <= 10) {
            let entity = &after[..semi];
            let decoded = match entity {
                "lt" => Some('<'),
                "gt" => Some('>'),
                "amp" => Some('&'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                    u32::from_str_radix(&entity[2..], 16).ok().and_then(char::from_u32)
                }
                _ if entity.starts_with('#') => entity[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            if let Some(ch) = decoded {
                out.push(ch);
                rest = &after[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((triple >> 6) & 0x3F) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(triple & 0x3F) as usize] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multistatus_calendar_query_and_cdata() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/work/ev-1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"abc-123"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR&#13;
BEGIN:VEVENT&#13;
UID:ev-1&#13;
SUMMARY:Standup &amp; Sync&#13;
END:VEVENT&#13;
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/user/work/ev-2.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>W/"xyz"</d:getetag>
        <c:calendar-data><![CDATA[BEGIN:VCALENDAR
BEGIN:VEVENT
UID:ev-2
SUMMARY:Cdata Event
END:VEVENT
END:VCALENDAR]]></c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

        let resources = parse_multistatus(xml);
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].href, "/calendars/user/work/ev-1.ics");
        assert_eq!(resources[0].etag.as_deref(), Some("\"abc-123\""));
        assert!(resources[0].status_ok);
        let data0 = resources[0].calendar_data.as_deref().unwrap();
        assert!(data0.contains("SUMMARY:Standup & Sync"));
        assert!(data0.contains("\r\n"));

        assert_eq!(resources[1].href, "/calendars/user/work/ev-2.ics");
        assert_eq!(normalize_etag(resources[1].etag.as_deref().unwrap()), "\"xyz\"");
        assert!(resources[1].calendar_data.as_deref().unwrap().contains("SUMMARY:Cdata Event"));
    }

    #[test]
    fn parses_calendar_discovery_and_privileges() {
        let xml = r#"<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <response>
    <href>/dav/calendars/alice/</href>
    <propstat>
      <prop>
        <resourcetype><collection/></resourcetype>
        <C:calendar-home-set><href>/dav/calendars/alice/</href></C:calendar-home-set>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
  <response>
    <href>/dav/calendars/alice/work/</href>
    <propstat>
      <prop>
        <resourcetype><collection/><C:calendar/></resourcetype>
        <displayname>Work</displayname>
        <current-user-privilege-set><privilege><read/></privilege></current-user-privilege-set>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
  <response>
    <href>/dav/calendars/alice/personal/</href>
    <propstat>
      <prop>
        <resourcetype><collection/><C:calendar/></resourcetype>
        <displayname>Personal</displayname>
        <current-user-privilege-set><privilege><read/></privilege><privilege><write/></privilege></current-user-privilege-set>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
</multistatus>"#;

        let resources = parse_multistatus(xml);
        assert_eq!(resources.len(), 3);
        assert_eq!(resources[0].calendar_home_set.as_deref(), Some("/dav/calendars/alice/"));
        assert!(!resources[0].is_calendar);
        assert!(resources[1].is_calendar && resources[1].read_only);
        assert_eq!(resources[1].display_name.as_deref(), Some("Work"));
        assert!(resources[2].is_calendar && !resources[2].read_only);
        assert_eq!(resources[2].display_name.as_deref(), Some("Personal"));
    }

    #[test]
    fn resolves_hrefs_and_builds_auth_headers() {
        assert_eq!(
            resolve_href("https://caldav.icloud.com/123/calendars/", "/123/calendars/work/a.ics"),
            "https://caldav.icloud.com/123/calendars/work/a.ics"
        );
        assert_eq!(
            resolve_href("https://dav.example.com/calendars/work/", "ev1.ics"),
            "https://dav.example.com/calendars/work/ev1.ics"
        );
        assert_eq!(event_href("https://dav.example.com/cal/", "abc/123"), "https://dav.example.com/cal/abc_123.ics");
        assert_eq!(auth_header("user@example.com", "secret").as_deref(), Some("Basic dXNlckBleGFtcGxlLmNvbTpzZWNyZXQ="));
        assert_eq!(auth_header("bearer", "tok_123").as_deref(), Some("Bearer tok_123"));
        assert_eq!(auth_header("", ""), None);
    }

    #[test]
    fn builds_multiget_with_escaped_hrefs() {
        let body = calendar_multiget_body(&["/cals/a&b.ics"]);
        assert!(body.contains("<d:href>/cals/a&amp;b.ics</d:href>"));
    }

    #[test]
    fn ignores_xml_comments_handles_split_cdata_and_direct_child_href() {
        let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:propstat>
      <d:prop>
        <d:current-user-principal><d:href>/dav/principals/alice/</d:href></d:current-user-principal>
        <d:resourcetype><!-- <c:calendar/> --><d:collection/></d:resourcetype>
        <d:current-user-privilege-set>
          <d:privilege><d:read/></d:privilege>
          <!-- <d:privilege><d:write/></d:privilege> -->
        </d:current-user-privilege-set>
        <c:calendar-data><![CDATA[BEGIN:VCALENDAR
SUMMARY:Part1 & ]]>amp; <![CDATA[Part2
END:VCALENDAR]]></c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
    <d:href>/dav/actual-response/</d:href>
  </d:response>
</d:multistatus>"#;

        let resources = parse_multistatus(xml);
        assert_eq!(resources.len(), 1);
        let r = &resources[0];
        assert_eq!(r.href, "/dav/actual-response/", "must not pick descendant <href> from <propstat>");
        assert_eq!(r.current_user_principal.as_deref(), Some("/dav/principals/alice/"));
        assert!(!r.is_calendar, "commented-out <calendar/> must be ignored");
        assert!(r.is_collection);
        assert!(r.read_only, "commented-out <write/> must not mark calendar writable");
        assert_eq!(
            r.calendar_data.as_deref(),
            Some("BEGIN:VCALENDAR\nSUMMARY:Part1 & amp; Part2\nEND:VCALENDAR")
        );
    }

    #[test]
    fn resolves_hrefs_with_query_fragment_dot_segments_and_protocol_relative() {
        assert_eq!(
            resolve_href("https://dav.example.com?token=1", "/cal/a.ics"),
            "https://dav.example.com/cal/a.ics"
        );
        assert_eq!(
            resolve_href("https://dav.example.com/a/b/c?x=1#y", "../d.ics?v=2#top"),
            "https://dav.example.com/a/d.ics?v=2#top"
        );
        assert_eq!(
            resolve_href("https://dav.example.com/a/b/#frag", "./ev.ics"),
            "https://dav.example.com/a/b/ev.ics"
        );
        assert_eq!(
            resolve_href("https://dav.example.com/a/b/", "//other.example.com/c.ics"),
            "https://other.example.com/c.ics"
        );
        assert_eq!(
            resolve_href("https://dav.example.com/cal/ev.ics?a=1#old", "?b=2"),
            "https://dav.example.com/cal/ev.ics?b=2"
        );
        assert_eq!(
            resolve_href("https://dav.example.com/cal/ev.ics?a=1#old", "#new"),
            "https://dav.example.com/cal/ev.ics?a=1#new"
        );
    }
}
