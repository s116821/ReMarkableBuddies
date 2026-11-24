use super::*;

fn assign_ids(value: &mut Value, next: &mut usize) {
    *next += 1;
    value["id"] = json!(format!("capacities-block-{next}"));
    if let Some(children) = value["blocks"].as_array_mut() {
        for child in children {
            assign_ids(child, next);
        }
    }
}

#[test]
fn renders_supported_markdown_into_native_readable_blocks() {
    let markdown = concat!(
        "# Heading [docs](https://example.com/docs) `run` $x$\n\n",
        "“Hello” -- **bold** *emphasis* ~~strike~~.\n\n",
        "1. [x] finished\n   - nested\n2. next\n\n",
        "> quoted\n>\n> second paragraph\n\n",
        "---\n\n",
        "```rust\nline(1);\n```\n\n",
        "    indented\n\n",
        "$$x+1$$\n",
    );
    let value = group(markdown).unwrap();
    let blocks = value["blocks"].as_array().unwrap();
    assert_eq!(value["type"], "GroupBlock");
    assert_eq!(blocks.len(), 8);
    assert_eq!(blocks[0]["hierarchy"], json!({"key":"Base","val":0}));
    assert_eq!(
        blocks[0]["tokens"][0],
        json!({"type":"TextToken","text":"Heading ","style":{"bold":true}})
    );
    assert_eq!(
        blocks[0]["tokens"][1],
        json!({"type":"LinkToken","text":"docs","url":"https://example.com/docs"})
    );
    assert_eq!(
        blocks[0]["tokens"][3],
        json!({"type":"CodeToken","text":"run"})
    );
    assert_eq!(
        blocks[0]["tokens"][5],
        json!({"type":"MathToken","text":"x"})
    );
    let tokens = blocks[1]["tokens"].as_array().unwrap();
    assert_eq!(tokens[0]["text"], "“Hello” -- ");
    for (word, style) in [
        ("bold", "bold"),
        ("emphasis", "italic"),
        ("strike", "strikethrough"),
    ] {
        assert!(tokens
            .iter()
            .any(|token| token["text"] == word && token["style"][style] == true));
    }
    assert_eq!(blocks[2]["blocks"][0]["list"], json!({"type":"numerical"}));
    assert_eq!(blocks[2]["blocks"][0]["todo"], json!({"isDone":true}));
    assert_eq!(
        blocks[2]["blocks"][0]["blocks"][0]["blocks"][0]["list"],
        json!({"type":"bullet"})
    );
    assert_eq!(blocks[2]["blocks"][1]["tokens"][0]["text"], "next");
    assert_eq!(blocks[3]["quote"], json!({"layout":"normal"}));
    assert_eq!(blocks[3]["tokens"][0]["text"], "quoted");
    assert_eq!(
        blocks[3]["blocks"][0]["tokens"][0]["text"],
        "second paragraph"
    );
    assert_eq!(blocks[4], json!({"type":"HorizontalLineBlock"}));
    assert_eq!(
        blocks[5],
        json!({"type":"CodeBlock","lang":"rust","text":"line(1);\n"})
    );
    assert_eq!(
        blocks[6],
        json!({"type":"CodeBlock","lang":"","text":"indented\n"})
    );
    assert_eq!(blocks[7], json!({"type":"MathBlock","text":"x+1"}));
}

#[test]
fn refuses_unsupported_features_and_nonrepresentable_styles() {
    let refused = [
        "<div>raw</div>",
        "inline <em>raw</em>",
        "![alt](https://example.com/icon.png)",
        "| A | B |\n| - | - |\n| 1 | 2 |\n",
        "Here[^1].\n\n[^1]: A footnote.\n",
        "3. skipped first numbers",
        "**[styled](https://example.com/)**",
        "[*styled label*](https://example.com/)",
        "**`styled code`**",
        "*$styled$*",
        "[invalid](javascript:alert)",
        "[relative](notes.md)",
        "[titled](https://example.com/ \"title\")",
        "```rust extra\ntext\n```\n",
        "x\0y",
    ];
    for markdown in refused {
        assert_eq!(group(markdown), Err(Failure::Unsupported), "{markdown:?}");
    }
}

#[test]
fn refuses_content_block_token_serialization_and_depth_overflows() {
    let overflow = [
        "x".repeat(MAX_CONTENT + 1),
        "---\n\n".repeat(MAX_BLOCKS),
        ("`x` ").repeat(MAX_TOKENS),
        "> ".repeat(MAX_DEPTH + 1) + "nested",
        "```\n".to_owned() + &"\t".repeat(145_000) + "code\n```\n",
    ];
    for markdown in overflow {
        assert_eq!(group(&markdown), Err(Failure::Unsupported));
    }
}

#[test]
fn readback_checks_exact_managed_schema_and_distinguishes_semantics_from_identity() {
    let desired =
        group("**Bold** plain\n\n- item\n\nA `code` [link](https://example.com/)").unwrap();
    let expected = semantic_digest(&desired).unwrap();
    let mut read = desired.clone();
    assign_ids(&mut read, &mut 0);
    let before = verify_group(&read).unwrap();
    assert_eq!(before.id, "capacities-block-1");
    assert_eq!(before.semantic_sha256, expected);
    read["blocks"][0]["list"] = Value::Null;
    read["blocks"][0]["toggle"] = Value::Null;
    read["blocks"][0]["tokens"][0]["style"]["italic"] = Value::Bool(false);
    let normalized = verify_group(&read).unwrap();
    assert_eq!(normalized.semantic_sha256, expected);
    assert_ne!(normalized.identity_sha256, before.identity_sha256);
    read["blocks"][0]["id"] = json!("another-server-block");
    let replaced = verify_group(&read).unwrap();
    assert_eq!(replaced.semantic_sha256, expected);
    assert_ne!(replaced.identity_sha256, normalized.identity_sha256);
    read["blocks"][0]["tokens"][0]["style"]["underline"] = Value::Bool(true);
    assert_ne!(verify_group(&read).unwrap().semantic_sha256, expected);
    read["blocks"][0]["id"] = json!("capacities-block-1");
    assert!(matches!(verify_group(&read), Err(Failure::InvalidResponse)));
}

#[test]
fn unknown_metadata_missing_ids_and_unsupported_tokens_refuse_readback() {
    let writable = group("text `code`").unwrap();
    assert!(matches!(
        verify_group(&writable),
        Err(Failure::InvalidResponse)
    ));
    let mut read = writable;
    assign_ids(&mut read, &mut 0);
    for (field, value) in [
        ("unknown", json!(0)),
        ("quote", Value::Null),
        ("list", json!({"type":"alphabetical"})),
        ("toggle", json!({"isOpen":false})),
    ] {
        let mut altered = read.clone();
        altered["blocks"][0][field] = value;
        assert!(matches!(
            verify_group(&altered),
            Err(Failure::InvalidResponse)
        ));
    }
    read["blocks"][0]["tokens"][1]["style"] = json!({});
    assert!(matches!(verify_group(&read), Err(Failure::InvalidResponse)));
}
