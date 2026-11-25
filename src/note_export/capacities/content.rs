//! Bounded writable Markdown mapping and strict managed-group readback.
use super::MAX_RESPONSE;
use crate::{
    note_export::{Failure, MAX_CONTENT, MAX_INPUT},
    storage::digest,
};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use serde_json::{json, Map, Value};
use std::{collections::BTreeSet, iter::Peekable};

type Result<T> = std::result::Result<T, Failure>;
const MAX_BLOCKS: usize = 4096;
const MAX_TOKENS: usize = 16_384;
const MAX_DEPTH: usize = 32;
const MAPPING_REVISION: u32 = 1;

pub(super) fn group(markdown: &str) -> Result<Value> {
    if markdown.is_empty() || markdown.len() > MAX_CONTENT || markdown.contains('\0') {
        return Err(Failure::Unsupported);
    }
    let options = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES;
    let mut mapper = Markdown {
        events: Parser::new_ext(markdown, options).peekable(),
        blocks: 0,
        tokens: 0,
    };
    let body = mapper.body(None, 1)?;
    if body.blocks.is_empty() {
        return Err(Failure::Unsupported);
    }
    let group = mapper.block(container(body.blocks))?;
    projected(&group, false).map_err(|_| Failure::Unsupported)?;
    Ok(group)
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Style {
    bold: bool,
    italic: bool,
    strike: bool,
}

impl Style {
    fn value(self, heading: bool) -> Value {
        let mut fields = Map::new();
        for (key, set) in [
            ("bold", self.bold || heading),
            ("italic", self.italic),
            ("strikethrough", self.strike),
        ] {
            if set {
                fields.insert(key.to_owned(), Value::Bool(true));
            }
        }
        Value::Object(fields)
    }
}

#[derive(Default)]
struct Body {
    blocks: Vec<Value>,
    task: Option<bool>,
}

struct Markdown<'a> {
    events: Peekable<Parser<'a>>,
    blocks: usize,
    tokens: usize,
}

impl Markdown<'_> {
    fn block(&mut self, block: Value) -> Result<Value> {
        self.blocks += 1;
        if self.blocks > MAX_BLOCKS {
            return Err(Failure::Unsupported);
        }
        Ok(block)
    }

    fn token(&mut self, tokens: &mut Vec<Value>, token: Value) -> Result<()> {
        self.tokens += 1;
        if self.tokens > MAX_TOKENS {
            return Err(Failure::Unsupported);
        }
        if token["type"] == "TextToken" {
            if let Some(last) = tokens
                .last_mut()
                .filter(|last| last["type"] == "TextToken" && last["style"] == token["style"])
            {
                if let (Some(Value::String(last)), Some(text)) =
                    (last.get_mut("text"), token["text"].as_str())
                {
                    last.push_str(text);
                    return Ok(());
                }
            }
        }
        tokens.push(token);
        Ok(())
    }

    fn body(&mut self, end: Option<TagEnd>, depth: usize) -> Result<Body> {
        if depth > MAX_DEPTH {
            return Err(Failure::Unsupported);
        }
        let item = end == Some(TagEnd::Item);
        let mut body = Body::default();
        while let Some(event) = self.events.peek() {
            if matches!(event, Event::End(found) if Some(*found) == end) {
                self.events.next();
                return Ok(body);
            }
            if item && begins_inline(event) {
                self.take_task(&mut body)?;
                self.paragraph(&mut body.blocks, None, false, depth)?;
                continue;
            }
            match self.events.next().ok_or(Failure::Unsupported)? {
                Event::Start(Tag::Paragraph) => {
                    if item {
                        self.take_task(&mut body)?;
                    }
                    self.paragraph(&mut body.blocks, Some(TagEnd::Paragraph), false, depth)?;
                }
                Event::Start(tag @ Tag::Heading { .. }) => {
                    self.paragraph(&mut body.blocks, Some(tag.to_end()), true, depth)?;
                }
                Event::Start(tag @ Tag::BlockQuote(None)) => {
                    let quoted = self.body(Some(tag.to_end()), depth + 1)?.blocks;
                    let mut quote = self.parent(quoted)?;
                    quote["quote"] = json!({"layout": "normal"});
                    body.blocks.push(quote);
                }
                Event::Start(Tag::List(start)) => {
                    body.blocks.push(self.list(start, depth + 1)?);
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    body.blocks.push(self.code_block(&kind)?);
                }
                Event::Rule => body
                    .blocks
                    .push(self.block(json!({"type": "HorizontalLineBlock"}))?),
                Event::DisplayMath(text) => body
                    .blocks
                    .push(self.block(json!({"type": "MathBlock", "text": text.as_ref()}))?),
                _ => return Err(Failure::Unsupported),
            }
        }
        if end.is_some() {
            return Err(Failure::Unsupported);
        }
        Ok(body)
    }

    fn take_task(&mut self, body: &mut Body) -> Result<()> {
        if let Some(Event::TaskListMarker(done)) = self.events.peek() {
            if body.task.is_some() || !body.blocks.is_empty() {
                return Err(Failure::Unsupported);
            }
            body.task = Some(*done);
            self.events.next();
        }
        Ok(())
    }

    fn list(&mut self, start: Option<u64>, depth: usize) -> Result<Value> {
        let kind = match start {
            None => "bullet",
            Some(1) => "numerical",
            _ => return Err(Failure::Unsupported),
        };
        let mut items = Vec::new();
        loop {
            match self.events.next() {
                Some(Event::Start(Tag::Item)) => {
                    let item = self.body(Some(TagEnd::Item), depth + 1)?;
                    let mut block = self.parent(item.blocks)?;
                    block["list"] = json!({"type": kind});
                    if let Some(done) = item.task {
                        block["todo"] = json!({"isDone": done});
                    }
                    items.push(block);
                }
                Some(Event::End(TagEnd::List(ordered))) if ordered == start.is_some() => break,
                _ => return Err(Failure::Unsupported),
            }
        }
        self.block(container(items))
    }

    fn parent(&mut self, mut body: Vec<Value>) -> Result<Value> {
        // A paragraph supplies the visible parent text. Preserve additional
        // paragraphs, lists, quotes and leaf blocks as ordered children.
        if body
            .first()
            .is_some_and(|block| block["type"] == "TextBlock" && block.get("quote").is_none())
        {
            let mut parent = body.remove(0);
            parent["blocks"] = Value::Array(body);
            Ok(parent)
        } else {
            self.block(text_block(Vec::new(), body))
        }
    }

    fn code_block(&mut self, kind: &CodeBlockKind<'_>) -> Result<Value> {
        let lang = match kind {
            CodeBlockKind::Indented => "",
            CodeBlockKind::Fenced(info) if !info.chars().any(char::is_whitespace) => info.as_ref(),
            CodeBlockKind::Fenced(_) => return Err(Failure::Unsupported),
        };
        let mut code = String::new();
        loop {
            match self.events.next() {
                Some(Event::Text(text)) => code.push_str(&text),
                Some(Event::End(TagEnd::CodeBlock)) => break,
                _ => return Err(Failure::Unsupported),
            }
        }
        self.block(json!({"type": "CodeBlock", "lang": lang, "text": code}))
    }

    fn paragraph(
        &mut self,
        blocks: &mut Vec<Value>,
        end: Option<TagEnd>,
        heading: bool,
        depth: usize,
    ) -> Result<()> {
        let mut tokens = Vec::new();
        loop {
            self.inline(&mut tokens, Style::default(), heading, None, depth)?;
            match self.events.peek() {
                Some(Event::DisplayMath(_)) if !heading => {
                    if !tokens.is_empty() {
                        blocks
                            .push(self.block(text_block(std::mem::take(&mut tokens), Vec::new()))?);
                    }
                    let Some(Event::DisplayMath(text)) = self.events.next() else {
                        return Err(Failure::Unsupported);
                    };
                    blocks.push(self.block(json!({"type": "MathBlock", "text": text.as_ref()}))?);
                }
                _ => break,
            }
        }
        if let Some(end) = end {
            if !matches!(self.events.next(), Some(Event::End(found)) if found == end) {
                return Err(Failure::Unsupported);
            }
        }
        if !tokens.is_empty() {
            blocks.push(self.block(text_block(tokens, Vec::new()))?);
        }
        Ok(())
    }

    fn inline(
        &mut self,
        tokens: &mut Vec<Value>,
        style: Style,
        heading: bool,
        end: Option<TagEnd>,
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(Failure::Unsupported);
        }
        while self.events.peek().is_some_and(begins_inline_span) {
            let token = match self.events.next().ok_or(Failure::Unsupported)? {
                Event::Text(text) => {
                    json!({"type": "TextToken", "text": text.as_ref(), "style": style.value(heading)})
                }
                Event::SoftBreak | Event::HardBreak => {
                    json!({"type": "TextToken", "text": "\n", "style": style.value(heading)})
                }
                Event::Code(text) if style == Style::default() => {
                    json!({"type": "CodeToken", "text": text.as_ref()})
                }
                Event::InlineMath(text) if style == Style::default() => {
                    json!({"type": "MathToken", "text": text.as_ref()})
                }
                Event::Start(Tag::Link {
                    dest_url, title, ..
                }) if style == Style::default() && title.is_empty() && ordinary_url(&dest_url) => {
                    self.link(&dest_url)?
                }
                Event::Start(tag @ (Tag::Emphasis | Tag::Strong | Tag::Strikethrough)) => {
                    let mut nested = style;
                    match tag {
                        Tag::Emphasis => nested.italic = true,
                        Tag::Strong => nested.bold = true,
                        Tag::Strikethrough => nested.strike = true,
                        _ => return Err(Failure::Unsupported),
                    }
                    self.inline(tokens, nested, heading, Some(tag.to_end()), depth + 1)?;
                    continue;
                }
                _ => return Err(Failure::Unsupported),
            };
            self.token(tokens, token)?;
        }
        if let Some(end) = end {
            if !matches!(self.events.next(), Some(Event::End(found)) if found == end) {
                return Err(Failure::Unsupported);
            }
        }
        Ok(())
    }

    fn link(&mut self, url: &str) -> Result<Value> {
        let mut label = String::new();
        loop {
            match self.events.next() {
                Some(Event::Text(text)) => label.push_str(&text),
                Some(Event::SoftBreak | Event::HardBreak) => label.push('\n'),
                Some(Event::End(TagEnd::Link)) => break,
                _ => return Err(Failure::Unsupported),
            }
        }
        Ok(json!({"type": "LinkToken", "text": label, "url": url}))
    }
}

fn begins_inline(event: &Event<'_>) -> bool {
    matches!(event, Event::TaskListMarker(_)) || begins_inline_span(event)
}

fn begins_inline_span(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Text(_)
            | Event::Code(_)
            | Event::InlineMath(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::FootnoteReference(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::Start(
                Tag::Emphasis
                    | Tag::Strong
                    | Tag::Strikethrough
                    | Tag::Link { .. }
                    | Tag::Image { .. }
                    | Tag::Superscript
                    | Tag::Subscript
            )
    )
}

fn ordinary_url(text: &str) -> bool {
    !text.is_empty()
        && !text.chars().any(|ch| ch.is_control() || ch.is_whitespace())
        && reqwest::Url::parse(text).is_ok_and(|url| match url.scheme() {
            "http" | "https" => {
                url.host_str().is_some() && url.username().is_empty() && url.password().is_none()
            }
            "mailto" => !url.path().is_empty(),
            _ => false,
        })
}

fn text_block(tokens: Vec<Value>, blocks: Vec<Value>) -> Value {
    json!({"type": "TextBlock", "tokens": tokens, "blocks": blocks, "hierarchy": {"key": "Base", "val": 0}})
}

fn container(blocks: Vec<Value>) -> Value {
    json!({"type": "GroupBlock", "blocks": blocks})
}

pub(super) struct VerifiedGroup {
    pub(super) id: String,
    pub(super) semantic_sha256: String,
    pub(super) identity_sha256: String,
}

pub(super) fn semantic_digest(group: &Value) -> Result<String> {
    let semantic = projected(group, false)?;
    fingerprint("semantics", semantic)
}

pub(super) fn verify_group(group: &Value) -> Result<VerifiedGroup> {
    let semantic = projected(group, true)?;
    Ok(VerifiedGroup {
        id: block_id(group)?.to_owned(),
        semantic_sha256: fingerprint("semantics", semantic)?,
        identity_sha256: fingerprint("identities", group.clone())?,
    })
}

fn fingerprint(domain: &str, mut value: Value) -> Result<String> {
    value.sort_all_objects();
    let encoded = serde_json::to_vec(&(domain, MAPPING_REVISION, value))
        .map_err(|_| Failure::InvalidResponse)?;
    Ok(digest(&encoded))
}

fn projected(group: &Value, read_ids: bool) -> Result<Value> {
    if group["type"] != "GroupBlock" {
        return Err(Failure::InvalidResponse);
    }
    let mut decoder = Projection {
        read_ids,
        ids: BTreeSet::new(),
        blocks: 0,
        tokens: 0,
    };
    let result = decoder.block(group, 1)?;
    let maximum = if read_ids { MAX_RESPONSE } else { MAX_INPUT };
    if serde_json::to_vec(group)
        .map_err(|_| Failure::InvalidResponse)?
        .len()
        > maximum
    {
        return Err(Failure::InvalidResponse);
    }
    Ok(result)
}

struct Projection<'a> {
    read_ids: bool,
    ids: BTreeSet<&'a str>,
    blocks: usize,
    tokens: usize,
}

impl<'a> Projection<'a> {
    fn block(&mut self, value: &'a Value, depth: usize) -> Result<Value> {
        self.blocks += 1;
        if self.blocks > MAX_BLOCKS || depth > MAX_DEPTH {
            return Err(Failure::InvalidResponse);
        }
        let block = object(value)?;
        if self.read_ids && !self.ids.insert(block_id(value)?) {
            return Err(Failure::InvalidResponse);
        }
        let fields: &[&str] = match value["type"].as_str() {
            Some("GroupBlock") if depth == 1 => &["blocks", "list", "toggle"],
            Some("GroupBlock") => &["blocks", "list", "todo", "toggle"],
            Some("TextBlock") => &[
                "tokens",
                "blocks",
                "hierarchy",
                "list",
                "todo",
                "toggle",
                "quote",
            ],
            Some("CodeBlock") => &["lang", "text"],
            Some("MathBlock") => &["text"],
            Some("HorizontalLineBlock") => &[],
            _ => return Err(Failure::InvalidResponse),
        };
        if block.keys().any(|key| {
            key != "type" && (key != "id" || !self.read_ids) && !fields.contains(&key.as_str())
        }) {
            return Err(Failure::InvalidResponse);
        }
        let mut semantic = Map::new();
        semantic.insert("type".to_owned(), value["type"].clone());
        match value["type"].as_str() {
            Some("TextBlock" | "GroupBlock") => {
                self.children(value, depth, &mut semantic)?;
                self.metadata(value, depth, &mut semantic)?;
                if value["type"] == "TextBlock" {
                    self.text_block(value, &mut semantic)?;
                }
            }
            Some("CodeBlock") => {
                semantic.insert(
                    "lang".to_owned(),
                    Value::String(string(&value["lang"])?.to_owned()),
                );
                semantic.insert(
                    "text".to_owned(),
                    Value::String(string(&value["text"])?.to_owned()),
                );
            }
            Some("MathBlock") => {
                semantic.insert(
                    "text".to_owned(),
                    Value::String(string(&value["text"])?.to_owned()),
                );
            }
            _ => {}
        }
        Ok(Value::Object(semantic))
    }

    fn children(
        &mut self,
        value: &'a Value,
        depth: usize,
        output: &mut Map<String, Value>,
    ) -> Result<()> {
        let blocks = array(&value["blocks"])?;
        if depth == 1 && blocks.is_empty() {
            return Err(Failure::InvalidResponse);
        }
        let blocks = blocks
            .iter()
            .map(|child| self.block(child, depth + 1))
            .collect::<Result<_>>()?;
        output.insert("blocks".to_owned(), Value::Array(blocks));
        Ok(())
    }

    fn metadata(&self, value: &Value, depth: usize, output: &mut Map<String, Value>) -> Result<()> {
        if !value["toggle"].is_null() || (!self.read_ids && value.get("toggle").is_some()) {
            return Err(Failure::InvalidResponse);
        }
        if let Some(list) = value.get("list") {
            if !(self.read_ids && list.is_null()) {
                if depth == 1
                    || !matches!(only(list, "type")?.as_str(), Some("bullet" | "numerical"))
                {
                    return Err(Failure::InvalidResponse);
                }
                output.insert("list".to_owned(), list.clone());
            }
        }
        if let Some(todo) = value.get("todo") {
            if !only(todo, "isDone")?.is_boolean() {
                return Err(Failure::InvalidResponse);
            }
            output.insert("todo".to_owned(), todo.clone());
        }
        if let Some(quote) = value.get("quote") {
            if only(quote, "layout")? != "normal" {
                return Err(Failure::InvalidResponse);
            }
            output.insert("quote".to_owned(), quote.clone());
        }
        Ok(())
    }

    fn text_block(&mut self, value: &Value, output: &mut Map<String, Value>) -> Result<()> {
        let hierarchy = json!({"key": "Base", "val": 0});
        if value["hierarchy"] != hierarchy {
            return Err(Failure::InvalidResponse);
        }
        output.insert("hierarchy".to_owned(), hierarchy);
        let tokens = array(&value["tokens"])?;
        self.tokens += tokens.len();
        if self.tokens > MAX_TOKENS {
            return Err(Failure::InvalidResponse);
        }
        let tokens = tokens.iter().map(semantic_token).collect::<Result<_>>()?;
        output.insert("tokens".to_owned(), Value::Array(tokens));
        Ok(())
    }
}

fn semantic_token(value: &Value) -> Result<Value> {
    let token = object(value)?;
    let text = string(&value["text"])?;
    match value["type"].as_str() {
        Some("TextToken") if token.len() == 3 => {
            let mut style = Map::new();
            for (key, enabled) in object(&value["style"])? {
                if !matches!(
                    key.as_str(),
                    "bold" | "italic" | "strikethrough" | "underline"
                ) {
                    return Err(Failure::InvalidResponse);
                }
                if enabled.as_bool().ok_or(Failure::InvalidResponse)? {
                    style.insert(key.clone(), Value::Bool(true));
                }
            }
            Ok(json!({"type": "TextToken", "text": text, "style": style}))
        }
        Some("LinkToken") if token.len() == 3 => {
            let url = string(&value["url"])?;
            if !ordinary_url(url) {
                return Err(Failure::InvalidResponse);
            }
            Ok(value.clone())
        }
        Some("CodeToken" | "MathToken") if token.len() == 2 => Ok(value.clone()),
        _ => Err(Failure::InvalidResponse),
    }
}

fn object(value: &Value) -> Result<&Map<String, Value>> {
    value.as_object().ok_or(Failure::InvalidResponse)
}

fn array(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or(Failure::InvalidResponse)
}

fn string(value: &Value) -> Result<&str> {
    value
        .as_str()
        .filter(|text| !text.contains('\0'))
        .ok_or(Failure::InvalidResponse)
}

fn only<'a>(value: &'a Value, field: &str) -> Result<&'a Value> {
    let fields = object(value)?;
    if fields.len() != 1 {
        return Err(Failure::InvalidResponse);
    }
    fields.get(field).ok_or(Failure::InvalidResponse)
}

fn block_id(value: &Value) -> Result<&str> {
    value["id"]
        .as_str()
        .filter(|id| (1..=1024).contains(&id.len()) && !id.chars().any(char::is_control))
        .ok_or(Failure::InvalidResponse)
}

#[cfg(test)]
#[path = "content_tests.rs"]
mod tests;
