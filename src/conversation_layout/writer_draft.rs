//! Plain tablet projection of a canonical Writer draft.
use anyhow::Result;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

pub(crate) fn writer_draft_text(markdown: &str, about: Option<&str>) -> Result<String> {
    let (title, body) = crate::writer::parse_draft(markdown)?;
    if let Some((about, body)) = about.and_then(|about| {
        body.strip_prefix(about)?
            .strip_prefix("\n\n")
            .map(|body| (about, body))
    }) {
        let heading = plain_markdown(&markdown[..title.len() + 2]);
        return Ok(format!("{heading}\n\n{about}\n\n{}", plain_markdown(body)));
    }
    Ok(plain_markdown(markdown))
}

fn plain_markdown(markdown: &str) -> String {
    let options = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_TABLES
        | Options::ENABLE_MATH
        | Options::ENABLE_FOOTNOTES;
    let mut text = String::with_capacity(markdown.len());
    let mut lists = Vec::new();
    let mut links = Vec::new();
    let mut cells = 0;

    for (event, range) in Parser::new_ext(markdown, options).into_offset_iter() {
        match event {
            Event::Text(content) => {
                let source = &markdown[range];
                // Retain ASCII entities that would otherwise introduce a
                // character the installed keyboard cannot type.
                text.push_str(if source.is_ascii() && !content.is_ascii() {
                    source
                } else {
                    &content
                });
            }
            Event::Code(content) | Event::Html(content) | Event::InlineHtml(content) => {
                text.push_str(&content);
            }
            Event::InlineMath(_) | Event::DisplayMath(_) => text.push_str(&markdown[range]),
            Event::Start(Tag::Strikethrough) => text.push_str("[crossed out: "),
            Event::End(TagEnd::Strikethrough) => text.push(']'),
            Event::Start(Tag::Emphasis) | Event::End(TagEnd::Emphasis)
                if markdown[range.clone()].starts_with('*')
                    && markdown[..range.start]
                        .chars()
                        .next_back()
                        .is_some_and(char::is_alphanumeric)
                    && markdown[range.end..]
                        .chars()
                        .next()
                        .is_some_and(char::is_alphanumeric) =>
            {
                // CommonMark also reads an unspaced product like 2*3*4 as emphasis.
                text.push('*');
            }
            Event::SoftBreak | Event::HardBreak => text.push('\n'),
            Event::Start(Tag::List(first)) => lists.push(first),
            Event::Start(Tag::Item) => {
                line_breaks(&mut text, 1);
                for _ in 1..lists.len() {
                    text.push_str("  ");
                }
                if let Some(Some(number)) = lists.last_mut() {
                    text.push_str(&number.to_string());
                    text.push_str(". ");
                    *number = number.saturating_add(1);
                } else {
                    text.push_str("- ");
                }
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
                line_breaks(&mut text, if lists.is_empty() { 2 } else { 1 });
            }
            Event::TaskListMarker(checked) => {
                text.push_str(if checked { "[x] " } else { "[ ] " });
            }
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                links.push((text.len(), dest_url));
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                if let Some((start, url)) = links.pop() {
                    if !url.is_empty() && &text[start..] != url.as_ref() {
                        if text.len() == start {
                            text.push_str(&url);
                        } else {
                            text.push_str(" (");
                            text.push_str(&url);
                            text.push(')');
                        }
                    }
                }
            }
            Event::Start(Tag::TableHead | Tag::TableRow) => cells = 0,
            Event::Start(Tag::TableCell) => {
                if cells != 0 {
                    text.push_str(" | ");
                }
                cells += 1;
            }
            Event::Start(Tag::FootnoteDefinition(label)) => {
                text.push('[');
                text.push_str(&label);
                text.push_str("] ");
            }
            Event::FootnoteReference(label) => {
                text.push('[');
                text.push_str(&label);
                text.push(']');
            }
            Event::Rule => {
                line_breaks(&mut text, 2);
                text.push_str("---");
                line_breaks(&mut text, 2);
            }
            Event::End(TagEnd::Item | TagEnd::TableHead | TagEnd::TableRow) => {
                line_breaks(&mut text, 1);
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::BlockQuote(_)
                | TagEnd::CodeBlock
                | TagEnd::HtmlBlock
                | TagEnd::Table
                | TagEnd::FootnoteDefinition,
            ) => line_breaks(&mut text, 2),
            _ => {}
        }
    }
    text.truncate(text.trim_end_matches('\n').len());
    text
}

fn line_breaks(text: &mut String, count: usize) {
    if !text.is_empty() {
        let existing = text.bytes().rev().take_while(|byte| *byte == b'\n').count();
        for _ in existing..count {
            text.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_projection_keeps_bound_about_math_and_literal_code() {
        let about = "About: the **V_1** label";
        let draft = format!(
            "# **Diagram**\n\n{about}\n\n## Inputs\n\n- **Pump** and ~~unused~~ Tank\n\
             - Use \\*literal\\*, $x_1 = y_2$, and 2*3*4.\n\n\
             `x * y` and [label](https://example.com)\n\n\
             ~~~rust\nlet s = \"**code**\";\n~~~"
        );
        let display = writer_draft_text(&draft, Some(about)).unwrap();
        assert_eq!(
            display,
            "Diagram\n\nAbout: the **V_1** label\n\nInputs\n\n- Pump and [crossed out: unused] Tank\n\
             - Use *literal*, $x_1 = y_2$, and 2*3*4.\n\n\
             x * y and label (https://example.com)\n\n\
             let s = \"**code**\";"
        );
    }

    #[test]
    fn writer_projection_distinguishes_crossed_out_and_corrected_values() {
        let draft = "# Measurements\n\nPressure: ~~10~~ 12 psi";
        assert_eq!(
            writer_draft_text(draft, None).unwrap(),
            "Measurements\n\nPressure: [crossed out: 10] 12 psi"
        );
    }
}
