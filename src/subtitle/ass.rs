#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssDocument {
    pub styles: Vec<AssStyle>,
    pub dialogues: Vec<AssDialogue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssStyle {
    pub name: String,
    pub font_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssDialogue {
    pub style: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Styles,
    Events,
    Other,
}

#[derive(Debug, Clone, Copy)]
struct StyleFormat {
    field_count: usize,
    name_index: usize,
    font_name_index: usize,
}

#[derive(Debug, Clone, Copy)]
struct EventFormat {
    field_count: usize,
    style_index: usize,
    text_index: usize,
}

pub fn parse_ass(text: &str) -> AssDocument {
    let mut document = AssDocument::default();
    let mut section = Section::Other;
    let mut style_format = None;
    let mut event_format = None;

    for line in text.lines() {
        let trimmed = line.trim();

        if let Some(next_section) = parse_section(trimmed) {
            section = next_section;
            continue;
        }

        match section {
            Section::Styles => {
                if let Some(rest) = strip_prefix_ci(trimmed, "Format:") {
                    style_format = parse_style_format(rest);
                    continue;
                }

                let Some(rest) = strip_prefix_ci(trimmed, "Style:") else {
                    continue;
                };
                let Some(format) = style_format else {
                    continue;
                };
                let Some(style) = parse_style_line(rest, format) else {
                    continue;
                };
                document.styles.push(style);
            }
            Section::Events => {
                if let Some(rest) = strip_prefix_ci(trimmed, "Format:") {
                    event_format = parse_event_format(rest);
                    continue;
                }

                let Some(rest) = strip_prefix_ci(trimmed, "Dialogue:") else {
                    continue;
                };
                let Some(format) = event_format else {
                    continue;
                };
                let Some(dialogue) = parse_dialogue_line(rest, format) else {
                    continue;
                };
                document.dialogues.push(dialogue);
            }
            Section::Other => {}
        }
    }

    document
}

fn parse_section(line: &str) -> Option<Section> {
    let section = line.strip_prefix('[')?.strip_suffix(']')?.trim();

    if section.eq_ignore_ascii_case("V4+ Styles") || section.eq_ignore_ascii_case("V4 Styles") {
        Some(Section::Styles)
    } else if section.eq_ignore_ascii_case("Events") {
        Some(Section::Events)
    } else {
        Some(Section::Other)
    }
}

fn parse_style_format(rest: &str) -> Option<StyleFormat> {
    let fields = parse_format_fields(rest);
    let name_index = find_field(&fields, "Name")?;
    let font_name_index = find_field(&fields, "Fontname")?;

    Some(StyleFormat {
        field_count: fields.len(),
        name_index,
        font_name_index,
    })
}

fn parse_event_format(rest: &str) -> Option<EventFormat> {
    let fields = parse_format_fields(rest);
    let style_index = find_field(&fields, "Style")?;
    let text_index = find_field(&fields, "Text")?;

    Some(EventFormat {
        field_count: fields.len(),
        style_index,
        text_index,
    })
}

fn parse_style_line(rest: &str, format: StyleFormat) -> Option<AssStyle> {
    let fields = split_fields(rest, format.field_count)?;

    Some(AssStyle {
        name: fields.get(format.name_index)?.trim().to_owned(),
        font_name: fields.get(format.font_name_index)?.trim().to_owned(),
    })
}

fn parse_dialogue_line(rest: &str, format: EventFormat) -> Option<AssDialogue> {
    let fields = split_fields(rest, format.field_count)?;

    Some(AssDialogue {
        style: fields.get(format.style_index)?.trim().to_owned(),
        text: fields.get(format.text_index)?.to_string(),
    })
}

fn parse_format_fields(rest: &str) -> Vec<String> {
    rest.split(',')
        .map(|field| field.trim().to_owned())
        .collect()
}

fn find_field(fields: &[String], name: &str) -> Option<usize> {
    fields
        .iter()
        .position(|field| field.eq_ignore_ascii_case(name))
}

fn split_fields(rest: &str, expected_field_count: usize) -> Option<Vec<&str>> {
    if expected_field_count == 0 {
        return None;
    }

    let fields = rest.splitn(expected_field_count, ',').collect::<Vec<_>>();
    (fields.len() == expected_field_count).then_some(fields)
}

fn strip_prefix_ci<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let actual_prefix = line.get(..prefix.len())?;

    if actual_prefix.eq_ignore_ascii_case(prefix) {
        line.get(prefix.len()..)
    } else {
        None
    }
}
