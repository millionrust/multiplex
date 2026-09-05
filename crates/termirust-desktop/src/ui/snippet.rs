//! Snippet placeholder helpers — pure string transforms, no UI state.

use crate::models::ConnectRequest;

pub fn extract_snippet_prompt_names(command: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut rest = command;
    while let Some(start) = rest.find("{{?") {
        let after = &rest[start + 3..];
        let Some(end_rel) = after.find("}}") else {
            break;
        };
        let name = after[..end_rel].trim().to_string();
        if !name.is_empty() && !names.iter().any(|n| n == &name) {
            names.push(name);
        }
        rest = &after[end_rel + 2..];
    }
    names
}

pub fn substitute_snippet_prompts(command: &str, values: &[(String, String)]) -> String {
    let mut out = String::with_capacity(command.len());
    let mut rest = command;
    while let Some(start) = rest.find("{{?") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        let Some(end_rel) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = after[..end_rel].trim();
        let replacement = values
            .iter()
            .find(|(prompt, _)| prompt == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        out.push_str(&replacement);
        rest = &after[end_rel + 2..];
    }
    out.push_str(rest);
    out
}

pub fn substitute_snippet_placeholders(command: &str, request: &ConnectRequest) -> String {
    let host = request.host.trim().to_string();
    let user = request.username.trim().to_string();
    let port = request.port.to_string();
    let title = request.title.trim().to_string();
    let address = request.address();

    let mut out = String::with_capacity(command.len());
    let mut rest = command;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        let Some(end_rel) = after_open.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = after_open[..end_rel].trim().to_ascii_uppercase();
        let replacement = match name.as_str() {
            "HOST" => Some(host.as_str()),
            "USER" | "USERNAME" => Some(user.as_str()),
            "PORT" => Some(port.as_str()),
            "TITLE" => Some(title.as_str()),
            "ADDRESS" => Some(address.as_str()),
            _ => None,
        };
        match replacement {
            Some(value) => out.push_str(value),
            None => {
                out.push_str("{{");
                out.push_str(&after_open[..end_rel]);
                out.push_str("}}");
            }
        }
        rest = &after_open[end_rel + 2..];
    }
    out.push_str(rest);
    out
}
