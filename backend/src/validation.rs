use anyhow::{bail, Result};

pub const MAX_TITLE_CHARS: usize = 120;
pub const MAX_BODY_BYTES: usize = 20_000;
pub const MAX_COMMENT_BYTES: usize = 20_000;
pub const MAX_TAGS: usize = 12;

pub fn primary_color(value: &str) -> Result<String> {
    let value = value.trim();
    if value.len() != 7
        || !value.starts_with('#')
        || !value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        bail!("primary_color must be a six-digit hex color");
    }
    Ok(value.to_ascii_lowercase())
}

pub fn slug(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 64
        || !value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
        || value.starts_with('-')
        || value.ends_with('-')
        || value.contains("--")
    {
        bail!("invalid slug");
    }
    Ok(value)
}

pub fn title(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_TITLE_CHARS {
        bail!("title must be between 1 and {MAX_TITLE_CHARS} characters");
    }
    Ok(value.to_owned())
}

pub fn body(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_BODY_BYTES {
        bail!("body must be between 1 and {MAX_BODY_BYTES} bytes");
    }
    Ok(value.to_owned())
}

pub fn comment(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_COMMENT_BYTES {
        bail!("comment must be between 1 and {MAX_COMMENT_BYTES} bytes");
    }
    Ok(value.to_owned())
}

pub fn category(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 64 || value.contains(['\n', '\r']) {
        bail!("invalid category");
    }
    Ok(value.to_owned())
}

pub fn tags(values: &[String]) -> Result<Vec<String>> {
    if values.len() > MAX_TAGS {
        bail!("too many tags");
    }
    let mut output = Vec::with_capacity(values.len());
    for value in values {
        let value = value.trim().to_ascii_lowercase();
        if value.is_empty() || value.len() > 32 || value.contains(['\n', '\r']) {
            bail!("invalid tag");
        }
        if !output.contains(&value) {
            output.push(value);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_lowercase_single_segments() {
        assert_eq!(slug("Acme-Board").unwrap(), "acme-board");
        assert!(slug("../admin").is_err());
        assert!(slug("a--b").is_err());
    }

    #[test]
    fn tags_are_normalized_and_deduplicated() {
        assert_eq!(tags(&["UX".into(), "ux".into()]).unwrap(), vec!["ux"]);
    }

    #[test]
    fn primary_colors_are_hex_only() {
        assert_eq!(primary_color(" #0099FF ").unwrap(), "#0099ff");
        assert!(primary_color("red; background: url(https://evil.test)").is_err());
    }
}
