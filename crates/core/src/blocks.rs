//! Block Kit: validating blocks and views the way Slack does (reporting
//! `[ERROR] … [json-pointer:…]` messages), filling in missing `block_id`s and
//! `action_id`s, and finding the interactive element a user acted on.

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use crate::ids::new_short_id;

/// Where blocks are shown. Limits differ between messages and views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Message,
    Modal,
    Home,
}

impl Surface {
    fn max_blocks(self) -> usize {
        match self {
            Surface::Message => 50,
            Surface::Modal | Surface::Home => 100,
        }
    }
}

const SELECTS: &[&str] = &[
    "static_select",
    "external_select",
    "users_select",
    "conversations_select",
    "channels_select",
    "multi_static_select",
    "multi_external_select",
    "multi_users_select",
    "multi_conversations_select",
    "multi_channels_select",
];

const TEXT_INPUTS: &[&str] = &[
    "plain_text_input",
    "email_text_input",
    "url_text_input",
    "number_input",
    "rich_text_input",
];

const PICKERS: &[&str] = &["datepicker", "timepicker", "datetimepicker"];

/// Element types that belong in a section's `accessory`.
fn accessory_ok(ty: &str) -> bool {
    SELECTS.contains(&ty)
        || matches!(
            ty,
            "button"
                | "checkboxes"
                | "datepicker"
                | "timepicker"
                | "image"
                | "overflow"
                | "radio_buttons"
                | "workflow_button"
        )
}

/// Element types allowed in an `actions` block.
fn action_ok(ty: &str) -> bool {
    SELECTS.contains(&ty)
        || PICKERS.contains(&ty)
        || matches!(
            ty,
            "button"
                | "checkboxes"
                | "overflow"
                | "radio_buttons"
                | "workflow_button"
                | "plain_text_input"
                | "rich_text_input"
        )
}

/// Element types allowed in an `input` block.
fn input_ok(ty: &str) -> bool {
    SELECTS.contains(&ty)
        || PICKERS.contains(&ty)
        || TEXT_INPUTS.contains(&ty)
        || matches!(ty, "checkboxes" | "radio_buttons" | "file_input")
}

fn known_element(ty: &str) -> bool {
    accessory_ok(ty) || action_ok(ty) || input_ok(ty)
}

/// Whether text objects in this position may be `mrkdwn` or only `plain_text`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TextKind {
    Any,
    Plain,
}

struct Validator {
    errors: Vec<String>,
    block_ids: HashSet<String>,
}

impl Validator {
    fn err(&mut self, pointer: &str, msg: impl AsRef<str>) {
        self.errors
            .push(format!("[ERROR] {} [json-pointer:{pointer}]", msg.as_ref()));
    }

    fn max_len(&mut self, v: &Value, pointer: &str, max: usize) {
        if let Some(s) = v.as_str()
            && s.chars().count() > max
        {
            self.err(pointer, format!("must be less than {} characters", max + 1));
        }
    }

    /// An optional string field with a length limit.
    fn opt_string(&mut self, obj: &Map<String, Value>, key: &str, pointer: &str, max: usize) {
        match obj.get(key) {
            None | Some(Value::Null) => {}
            Some(v @ Value::String(_)) => self.max_len(v, &format!("{pointer}/{key}"), max),
            Some(_) => self.err(&format!("{pointer}/{key}"), "must be a string"),
        }
    }

    fn req_string(&mut self, obj: &Map<String, Value>, key: &str, pointer: &str, max: usize) {
        match obj.get(key) {
            Some(Value::String(s)) if !s.is_empty() => self.max_len(&obj[key], &format!("{pointer}/{key}"), max),
            Some(Value::String(_)) => self.err(&format!("{pointer}/{key}"), "must be more than 0 characters"),
            Some(_) => self.err(&format!("{pointer}/{key}"), "must be a string"),
            None => self.err(pointer, format!("missing required field: {key}")),
        }
    }

    fn text(&mut self, v: &Value, pointer: &str, kind: TextKind, max: usize) {
        let Some(obj) = v.as_object() else {
            self.err(pointer, "must be a text object");
            return;
        };
        match obj.get("type").and_then(Value::as_str) {
            Some("plain_text") => {}
            Some("mrkdwn") if kind == TextKind::Any => {}
            Some("mrkdwn") => self.err(&format!("{pointer}/type"), "must be plain_text"),
            Some(other) => self.err(&format!("{pointer}/type"), format!("unsupported type: {other}")),
            None => self.err(pointer, "missing required field: type"),
        }
        self.req_string(obj, "text", pointer, max);
    }

    fn opt_text(&mut self, obj: &Map<String, Value>, key: &str, pointer: &str, kind: TextKind, max: usize) {
        if let Some(v) = obj.get(key).filter(|v| !v.is_null()) {
            self.text(v, &format!("{pointer}/{key}"), kind, max);
        }
    }

    fn req_text(&mut self, obj: &Map<String, Value>, key: &str, pointer: &str, kind: TextKind, max: usize) {
        match obj.get(key).filter(|v| !v.is_null()) {
            Some(v) => self.text(v, &format!("{pointer}/{key}"), kind, max),
            None => self.err(pointer, format!("missing required field: {key}")),
        }
    }

    fn array<'a>(
        &mut self,
        obj: &'a mut Map<String, Value>,
        key: &str,
        pointer: &str,
        min: usize,
        max: usize,
    ) -> Option<&'a mut Vec<Value>> {
        match obj.get_mut(key) {
            Some(Value::Array(a)) => {
                if a.len() < min {
                    self.errors.push(format!(
                        "[ERROR] must provide at least {min} items [json-pointer:{pointer}/{key}]"
                    ));
                } else if a.len() > max {
                    self.errors.push(format!(
                        "[ERROR] must provide no more than {max} items [json-pointer:{pointer}/{key}]"
                    ));
                }
                Some(a)
            }
            Some(_) => {
                self.err(&format!("{pointer}/{key}"), "must be an array");
                None
            }
            None => {
                if min > 0 {
                    self.err(pointer, format!("missing required field: {key}"));
                }
                None
            }
        }
    }

    fn option(&mut self, v: &Value, pointer: &str, kind: TextKind) {
        let Some(obj) = v.as_object() else {
            self.err(pointer, "must be an option object");
            return;
        };
        self.req_text(obj, "text", pointer, kind, 75);
        self.req_string(obj, "value", pointer, 150);
        self.opt_text(obj, "description", pointer, kind, 75);
    }

    fn options(&mut self, el: &mut Map<String, Value>, pointer: &str, min: usize, max: usize, kind: TextKind) {
        if let Some(opts) = self.array(el, "options", pointer, min, max) {
            let opts = opts.clone();
            for (i, o) in opts.iter().enumerate() {
                self.option(o, &format!("{pointer}/options/{i}"), kind);
            }
        }
    }

    fn confirm(&mut self, el: &Map<String, Value>, pointer: &str) {
        let Some(c) = el.get("confirm").filter(|v| !v.is_null()) else {
            return;
        };
        let pointer = format!("{pointer}/confirm");
        let Some(obj) = c.as_object() else {
            self.err(&pointer, "must be a confirmation dialog object");
            return;
        };
        for key in ["title", "confirm", "deny"] {
            self.opt_text(obj, key, &pointer, TextKind::Plain, 100);
        }
        self.req_text(obj, "text", &pointer, TextKind::Any, 300);
    }

    /// Validates one element and fills in its `action_id`.
    fn element(&mut self, v: &mut Value, pointer: &str, allowed: fn(&str) -> bool, action_ids: &mut HashSet<String>) {
        let Some(el) = v.as_object_mut() else {
            self.err(pointer, "must be an element object");
            return;
        };
        let Some(ty) = el.get("type").and_then(Value::as_str).map(str::to_owned) else {
            self.err(pointer, "missing required field: type");
            return;
        };
        if !known_element(&ty) {
            self.err(&format!("{pointer}/type"), format!("unsupported type: {ty}"));
            return;
        }
        if !allowed(&ty) {
            self.err(&format!("{pointer}/type"), format!("{ty} is not allowed here"));
            return;
        }
        if ty == "image" {
            self.image(el, pointer);
            return;
        }
        match el.get("action_id") {
            None | Some(Value::Null) => {
                el.insert("action_id".into(), json!(new_short_id(5)));
            }
            Some(Value::String(id)) if id.chars().count() <= 255 => {
                if !action_ids.insert(id.clone()) {
                    self.err(
                        &format!("{pointer}/action_id"),
                        format!("action_id must be unique within a block: {id}"),
                    );
                }
            }
            Some(_) => self.err(
                &format!("{pointer}/action_id"),
                "must be a string of at most 255 characters",
            ),
        }
        self.opt_text(el, "placeholder", pointer, TextKind::Plain, 150);
        self.confirm(el, pointer);
        match ty.as_str() {
            "button" | "workflow_button" => {
                self.req_text(el, "text", pointer, TextKind::Plain, 75);
                self.opt_string(el, "value", pointer, 2000);
                self.opt_string(el, "url", pointer, 3000);
                if let Some(style) = el.get("style").filter(|v| !v.is_null())
                    && !matches!(style.as_str(), Some("primary" | "danger"))
                {
                    self.err(&format!("{pointer}/style"), "must be one of: primary, danger");
                }
            }
            "static_select" | "multi_static_select" => {
                let has_groups = el.contains_key("option_groups");
                if el.contains_key("options") == has_groups {
                    self.err(pointer, "must provide exactly one of: options, option_groups");
                } else if has_groups {
                    if let Some(groups) = self.array(el, "option_groups", pointer, 1, 100) {
                        let groups = groups.clone();
                        for (g, group) in groups.iter().enumerate() {
                            let gp = format!("{pointer}/option_groups/{g}");
                            let mut group = group.clone();
                            if let Some(obj) = group.as_object_mut() {
                                self.req_text(obj, "label", &gp, TextKind::Plain, 75);
                                self.options(obj, &gp, 1, 100, TextKind::Plain);
                            }
                        }
                    }
                } else {
                    self.options(el, pointer, 1, 100, TextKind::Plain);
                }
            }
            "overflow" => self.options(el, pointer, 1, 5, TextKind::Plain),
            "checkboxes" | "radio_buttons" => self.options(el, pointer, 1, 10, TextKind::Any),
            "plain_text_input" => {
                self.opt_string(el, "initial_value", pointer, 3000);
            }
            "number_input" if !el.get("is_decimal_allowed").is_some_and(Value::is_boolean) => {
                self.err(pointer, "missing required field: is_decimal_allowed");
            }
            "datepicker" => self.pattern(el, "initial_date", pointer, is_date, "YYYY-MM-DD"),
            "timepicker" => self.pattern(el, "initial_time", pointer, is_time, "HH:mm"),
            _ => {}
        }
    }

    fn pattern(&mut self, el: &Map<String, Value>, key: &str, pointer: &str, ok: fn(&str) -> bool, fmt: &str) {
        if let Some(v) = el.get(key).filter(|v| !v.is_null())
            && !v.as_str().is_some_and(ok)
        {
            self.err(&format!("{pointer}/{key}"), format!("must be in the format {fmt}"));
        }
    }

    fn image(&mut self, obj: &Map<String, Value>, pointer: &str) {
        if !obj.contains_key("image_url") && !obj.contains_key("slack_file") {
            self.err(pointer, "missing required field: image_url");
        }
        self.opt_string(obj, "image_url", pointer, 3000);
        self.req_string(obj, "alt_text", pointer, 2000);
    }

    fn block(&mut self, v: &mut Value, pointer: &str) {
        let Some(b) = v.as_object_mut() else {
            self.err(pointer, "must be a block object");
            return;
        };
        let Some(ty) = b.get("type").and_then(Value::as_str).map(str::to_owned) else {
            self.err(pointer, "missing required field: type");
            return;
        };
        match b.get("block_id") {
            None | Some(Value::Null) => {
                b.insert("block_id".into(), json!(new_short_id(5)));
            }
            Some(Value::String(id)) if !id.is_empty() && id.chars().count() <= 255 => {
                if !self.block_ids.insert(id.clone()) {
                    self.err(&format!("{pointer}/block_id"), format!("block_id must be unique: {id}"));
                }
            }
            Some(_) => self.err(
                &format!("{pointer}/block_id"),
                "must be a string of 1 to 255 characters",
            ),
        }
        let mut action_ids = HashSet::new();
        match ty.as_str() {
            "section" => {
                if !b.contains_key("text") && !b.contains_key("fields") {
                    self.err(pointer, "must provide at least one of: text, fields");
                }
                self.opt_text(b, "text", pointer, TextKind::Any, 3000);
                if b.contains_key("fields")
                    && let Some(fields) = self.array(b, "fields", pointer, 1, 10)
                {
                    let fields = fields.clone();
                    for (i, f) in fields.iter().enumerate() {
                        self.text(f, &format!("{pointer}/fields/{i}"), TextKind::Any, 2000);
                    }
                }
                if let Some(acc) = b.get_mut("accessory") {
                    self.element(acc, &format!("{pointer}/accessory"), accessory_ok, &mut action_ids);
                }
            }
            "divider" => {}
            "header" => self.req_text(b, "text", pointer, TextKind::Plain, 150),
            "image" => {
                self.image(b, pointer);
                self.opt_text(b, "title", pointer, TextKind::Plain, 2000);
            }
            "context" => {
                if let Some(els) = self.array(b, "elements", pointer, 1, 10) {
                    let mut els = std::mem::take(els);
                    for (i, el) in els.iter_mut().enumerate() {
                        let p = format!("{pointer}/elements/{i}");
                        match el["type"].as_str() {
                            Some("image") => self.image(el.as_object().expect("has type"), &p),
                            _ => self.text(el, &p, TextKind::Any, 3000),
                        }
                    }
                    b.insert("elements".into(), Value::Array(els));
                }
            }
            "actions" => {
                if let Some(els) = self.array(b, "elements", pointer, 1, 25) {
                    let mut els = std::mem::take(els);
                    for (i, el) in els.iter_mut().enumerate() {
                        self.element(el, &format!("{pointer}/elements/{i}"), action_ok, &mut action_ids);
                    }
                    b.insert("elements".into(), Value::Array(els));
                }
            }
            "input" => {
                self.req_text(b, "label", pointer, TextKind::Plain, 2000);
                self.opt_text(b, "hint", pointer, TextKind::Plain, 2000);
                match b.get_mut("element") {
                    Some(el) => self.element(el, &format!("{pointer}/element"), input_ok, &mut action_ids),
                    None => self.err(pointer, "missing required field: element"),
                }
            }
            "rich_text" => {
                self.array(b, "elements", pointer, 0, usize::MAX);
            }
            "video" => {
                for key in ["video_url", "thumbnail_url", "alt_text"] {
                    self.req_string(b, key, pointer, 3000);
                }
                self.req_text(b, "title", pointer, TextKind::Plain, 200);
            }
            "markdown" => self.req_string(b, "text", pointer, 12000),
            "file" => {
                self.req_string(b, "external_id", pointer, 255);
                self.req_string(b, "source", pointer, 255);
            }
            // Accepted without detailed checks.
            "table" | "call" | "context_actions" => {}
            other => self.err(&format!("{pointer}/type"), format!("unsupported type: {other}")),
        }
    }
}

/// Validates `blocks` (a JSON array) in place, filling in missing `block_id`s
/// and `action_id`s. Returns Slack-style error messages; empty means valid.
pub fn validate_blocks(blocks: &mut Value, surface: Surface, pointer: &str) -> Vec<String> {
    let mut v = Validator {
        errors: Vec::new(),
        block_ids: HashSet::new(),
    };
    let Some(list) = blocks.as_array_mut() else {
        v.err(pointer, "must be an array");
        return v.errors;
    };
    if list.len() > surface.max_blocks() {
        v.err(
            pointer,
            format!("must provide no more than {} items", surface.max_blocks()),
        );
    }
    for (i, b) in list.iter_mut().enumerate() {
        v.block(b, &format!("{pointer}/{i}"));
    }
    v.errors
}

/// Validates a view (`views.open` / `push` / `update` / `publish`) in place.
/// `expected` restricts the view type (e.g. `home` for `views.publish`).
pub fn validate_view(view: &mut Value, expected: Option<&str>) -> Vec<String> {
    let pointer = "/view";
    let mut v = Validator {
        errors: Vec::new(),
        block_ids: HashSet::new(),
    };
    let Some(obj) = view.as_object_mut() else {
        v.err(pointer, "must be an object");
        return v.errors;
    };
    let ty = obj.get("type").and_then(Value::as_str).unwrap_or_default().to_owned();
    let surface = match ty.as_str() {
        "modal" => Surface::Modal,
        "home" => Surface::Home,
        "" => {
            v.err(pointer, "missing required field: type");
            return v.errors;
        }
        other => {
            v.err(&format!("{pointer}/type"), format!("unsupported type: {other}"));
            return v.errors;
        }
    };
    if let Some(want) = expected
        && want != ty
    {
        v.err(&format!("{pointer}/type"), format!("must be {want}"));
    }
    if surface == Surface::Modal {
        v.req_text(obj, "title", pointer, TextKind::Plain, 24);
    } else {
        v.opt_text(obj, "title", pointer, TextKind::Plain, 24);
    }
    v.opt_text(obj, "submit", pointer, TextKind::Plain, 24);
    v.opt_text(obj, "close", pointer, TextKind::Plain, 24);
    v.opt_string(obj, "private_metadata", pointer, 3000);
    v.opt_string(obj, "callback_id", pointer, 255);
    v.opt_string(obj, "external_id", pointer, 255);
    let mut errors = v.errors;
    match obj.get_mut("blocks") {
        Some(blocks) => {
            errors.extend(validate_blocks(blocks, surface, &format!("{pointer}/blocks")));
            if surface == Surface::Modal && has_input_blocks(blocks) && obj.get("submit").is_none_or(Value::is_null) {
                errors.push(format!(
                    "[ERROR] must provide a submit property when using input blocks [json-pointer:{pointer}]"
                ));
            }
        }
        None => errors.push(format!(
            "[ERROR] missing required field: blocks [json-pointer:{pointer}]"
        )),
    }
    errors
}

pub fn has_input_blocks(blocks: &Value) -> bool {
    blocks
        .as_array()
        .is_some_and(|bs| bs.iter().any(|b| b["type"] == "input"))
}

/// The interactive elements of a block.
fn block_elements(block: &Value) -> Vec<&Value> {
    match block["type"].as_str() {
        Some("section") => block.get("accessory").into_iter().collect(),
        Some("actions") => block["elements"]
            .as_array()
            .map(|a| a.iter().collect())
            .unwrap_or_default(),
        Some("input") => block.get("element").into_iter().collect(),
        _ => Vec::new(),
    }
}

/// Finds the element with this `block_id` / `action_id`. Looks in the blocks of
/// legacy attachments too, when given a message's `attachments`.
pub fn find_element<'a>(blocks: &'a Value, block_id: &str, action_id: &str) -> Option<&'a Value> {
    blocks.as_array()?.iter().find_map(|b| {
        if b["block_id"] != block_id {
            return None;
        }
        block_elements(b).into_iter().find(|el| el["action_id"] == action_id)
    })
}

/// Keeps only the state values whose block and action still exist (as Slack
/// does when a view is updated).
pub fn retain_state(values: &mut Map<String, Value>, blocks: &Value) {
    let ids: HashSet<(String, String)> = blocks
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|b| {
            let block_id = b["block_id"].as_str().unwrap_or_default().to_owned();
            block_elements(b)
                .into_iter()
                .filter_map(move |el| Some((block_id.clone(), el["action_id"].as_str()?.to_owned())))
        })
        .collect();
    values.retain(|block_id, actions| {
        if let Some(actions) = actions.as_object_mut() {
            actions.retain(|action_id, _| ids.contains(&(block_id.clone(), action_id.clone())));
            !actions.is_empty()
        } else {
            false
        }
    });
}

/// The check Slack's client makes before submitting a modal: every required
/// `input` has a value. Returns `block_id` → message for those that don't.
pub fn check_view_values(blocks: &Value, values: &Map<String, Value>) -> Option<Map<String, Value>> {
    let mut errors = Map::new();
    for b in blocks.as_array().into_iter().flatten() {
        if b["type"] != "input" || b["optional"] == true {
            continue;
        }
        let (Some(block_id), Some(action_id)) = (b["block_id"].as_str(), b["element"]["action_id"].as_str()) else {
            continue;
        };
        let value = values.get(block_id).and_then(|a| a.get(action_id));
        if !value.is_some_and(has_value) {
            errors.insert(block_id.into(), json!("Please complete this required field."));
        }
    }
    (!errors.is_empty()).then_some(errors)
}

fn has_value(v: &Value) -> bool {
    const SINGLE: &[&str] = &[
        "value",
        "selected_option",
        "selected_user",
        "selected_conversation",
        "selected_channel",
        "selected_date",
        "selected_time",
        "selected_date_time",
        "rich_text_value",
    ];
    const MULTI: &[&str] = &[
        "selected_options",
        "selected_users",
        "selected_conversations",
        "selected_channels",
        "files",
    ];
    SINGLE
        .iter()
        .any(|k| v.get(*k).is_some_and(|x| !x.is_null() && x != ""))
        || MULTI
            .iter()
            .any(|k| v.get(*k).and_then(Value::as_array).is_some_and(|a| !a.is_empty()))
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

fn is_time(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 5 && b[2] == b':' && b.iter().enumerate().all(|(i, c)| i == 2 || c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_ids_and_accepts_common_blocks() {
        let mut blocks = json!([
            {"type": "header", "text": {"type": "plain_text", "text": "Deploy"}},
            {"type": "section", "text": {"type": "mrkdwn", "text": "*Ready?*"},
             "accessory": {"type": "static_select", "placeholder": {"type": "plain_text", "text": "Env"},
                           "options": [{"text": {"type": "plain_text", "text": "prod"}, "value": "prod"}]}},
            {"type": "divider"},
            {"type": "actions", "block_id": "go", "elements": [
                {"type": "button", "text": {"type": "plain_text", "text": "Ship"}, "style": "primary", "action_id": "ship"},
                {"type": "button", "text": {"type": "plain_text", "text": "Cancel"}}
            ]},
            {"type": "context", "elements": [{"type": "mrkdwn", "text": "by <@U1>"}]}
        ]);
        let errors = validate_blocks(&mut blocks, Surface::Message, "/blocks");
        assert!(errors.is_empty(), "{errors:?}");
        assert!(blocks[0]["block_id"].is_string());
        assert!(blocks[1]["accessory"]["action_id"].is_string());
        assert!(blocks[3]["elements"][1]["action_id"].is_string());
        assert_eq!(find_element(&blocks, "go", "ship").unwrap()["style"], "primary");
    }

    #[test]
    fn reports_slack_style_errors() {
        let mut blocks = json!([
            {"type": "sectoin", "text": {"type": "mrkdwn", "text": "x"}},
            {"type": "section"},
            {"type": "header", "text": {"type": "mrkdwn", "text": "x"}},
            {"type": "actions", "elements": [{"type": "button", "text": {"type": "plain_text", "text": ""}}]},
            {"type": "divider", "block_id": "a"},
            {"type": "divider", "block_id": "a"}
        ]);
        let errors = validate_blocks(&mut blocks, Surface::Message, "/blocks");
        assert_eq!(
            errors,
            [
                "[ERROR] unsupported type: sectoin [json-pointer:/blocks/0/type]",
                "[ERROR] must provide at least one of: text, fields [json-pointer:/blocks/1]",
                "[ERROR] must be plain_text [json-pointer:/blocks/2/text/type]",
                "[ERROR] must be more than 0 characters [json-pointer:/blocks/3/elements/0/text/text]",
                "[ERROR] block_id must be unique: a [json-pointer:/blocks/5/block_id]",
            ]
        );
    }

    #[test]
    fn views() {
        let mut modal = json!({
            "type": "modal",
            "title": {"type": "plain_text", "text": "New ticket"},
            "blocks": [{"type": "input", "label": {"type": "plain_text", "text": "Title"},
                        "element": {"type": "plain_text_input"}}]
        });
        assert_eq!(
            validate_view(&mut modal, None),
            ["[ERROR] must provide a submit property when using input blocks [json-pointer:/view]"]
        );
        modal["submit"] = json!({"type": "plain_text", "text": "Create"});
        assert!(validate_view(&mut modal, None).is_empty());
        assert_eq!(
            validate_view(&mut modal, Some("home")),
            ["[ERROR] must be home [json-pointer:/view/type]"]
        );
        let mut home = json!({"type": "home", "blocks": []});
        assert!(validate_view(&mut home, Some("home")).is_empty());
    }

    #[test]
    fn required_inputs() {
        let blocks = json!([
            {"type": "input", "block_id": "t", "element": {"type": "plain_text_input", "action_id": "v"}},
            {"type": "input", "block_id": "o", "optional": true, "element": {"type": "plain_text_input", "action_id": "v"}},
            {"type": "input", "block_id": "s", "element": {"type": "multi_users_select", "action_id": "v"}}
        ]);
        let values =
            json!({"t": {"v": {"type": "plain_text_input", "value": "hi"}}, "s": {"v": {"selected_users": []}}});
        let errors = check_view_values(&blocks, values.as_object().unwrap()).unwrap();
        assert_eq!(errors.keys().collect::<Vec<_>>(), ["s"]);
    }

    #[test]
    fn retains_matching_state() {
        let blocks =
            json!([{"type": "input", "block_id": "b", "element": {"type": "plain_text_input", "action_id": "a"}}]);
        let mut values = json!({"b": {"a": {"value": "keep"}, "gone": {}}, "old": {"x": {}}});
        retain_state(values.as_object_mut().unwrap(), &blocks);
        assert_eq!(values, json!({"b": {"a": {"value": "keep"}}}));
    }
}
