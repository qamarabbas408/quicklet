//! Default output filename policy.
//!
//! Timestamps use local system time and the compact, sortable
//! `YYYYMMDD-HHMMSS` format. Timestamps are naming conveniences only; they do
//! not replace native overwrite confirmation or no-clobber persistence.

use chrono::{DateTime, Local};

use crate::compress::CompressionPreset;

const MAX_SOURCE_STEM_BYTES: usize = 120;

pub fn default_output_name(
    action_id: &str,
    compression_preset: Option<CompressionPreset>,
    now: DateTime<Local>,
) -> Option<String> {
    let action = match action_id {
        "images.convert_to_pdf" => "Images".to_string(),
        "pdf.merge" => "Merge".to_string(),
        "pdf.compress" => {
            format!("Compress-{}", compression_preset?.label())
        }
        _ => return None,
    };

    Some(format!("Quicklet-{action}-{}.pdf", format_timestamp(now)))
}

pub fn split_output_name(stem: &str, start: usize, end: usize) -> String {
    let stem = sanitize_source_stem(stem);
    if start == end {
        format!("Quicklet-Split-{stem}-page-{start:03}.pdf")
    } else {
        format!("Quicklet-Split-{stem}-pages-{start:03}-{end:03}.pdf")
    }
}

pub fn format_timestamp(now: DateTime<Local>) -> String {
    now.format("%Y%m%d-%H%M%S").to_string()
}

fn sanitize_source_stem(stem: &str) -> String {
    let mut value = stem.trim();

    while value.len() >= 4 && value.as_bytes()[value.len() - 4..].eq_ignore_ascii_case(b".pdf") {
        value = &value[..value.len() - 4];
        value = value.trim();
    }

    let mut cleaned = String::new();
    let mut previous_was_separator = false;

    for character in value.chars() {
        let invalid = character.is_control()
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            );
        let character = if invalid { '-' } else { character };

        if character == '-' {
            if previous_was_separator {
                continue;
            }
            previous_was_separator = true;
        } else {
            previous_was_separator = false;
        }

        if cleaned.len() + character.len_utf8() > MAX_SOURCE_STEM_BYTES {
            break;
        }
        cleaned.push(character);
    }

    let mut cleaned = cleaned.trim_matches(|character: char| {
        character.is_whitespace() || character == '.' || character == '-'
    });

    while cleaned.len() >= 4
        && cleaned.as_bytes()[cleaned.len() - 4..].eq_ignore_ascii_case(b".pdf")
    {
        cleaned = &cleaned[..cleaned.len() - 4];
        cleaned = cleaned.trim_matches(|character: char| {
            character.is_whitespace() || character == '.' || character == '-'
        });
    }

    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "Document".to_string()
    } else {
        cleaned.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fixed_time() -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 10, 24, 15, 30, 45)
            .single()
            .expect("fixed local time should be valid")
    }

    #[test]
    fn builds_action_specific_default_names() {
        let now = fixed_time();

        assert_eq!(
            default_output_name("images.convert_to_pdf", None, now).as_deref(),
            Some("Quicklet-Images-20261024-153045.pdf")
        );
        assert_eq!(
            default_output_name("pdf.merge", None, now).as_deref(),
            Some("Quicklet-Merge-20261024-153045.pdf")
        );
        assert_eq!(
            default_output_name("pdf.compress", Some(CompressionPreset::Light), now).as_deref(),
            Some("Quicklet-Compress-Light-20261024-153045.pdf")
        );
        assert_eq!(
            default_output_name("pdf.compress", Some(CompressionPreset::Balanced), now).as_deref(),
            Some("Quicklet-Compress-Balanced-20261024-153045.pdf")
        );
        assert_eq!(
            default_output_name("pdf.compress", Some(CompressionPreset::Strong), now).as_deref(),
            Some("Quicklet-Compress-Strong-20261024-153045.pdf")
        );
    }

    #[test]
    fn timestamp_format_and_same_second_behavior_are_stable() {
        let now = fixed_time();
        assert_eq!(format_timestamp(now), "20261024-153045");
        assert_eq!(format_timestamp(now).len(), 15);

        let padded = Local
            .with_ymd_and_hms(2026, 1, 5, 3, 4, 5)
            .single()
            .unwrap();
        assert_eq!(format_timestamp(padded), "20260105-030405");

        assert_eq!(
            default_output_name("pdf.merge", None, now),
            default_output_name("pdf.merge", None, now)
        );
    }

    #[test]
    fn refuses_split_and_unknown_single_file_actions() {
        let now = fixed_time();
        assert!(default_output_name("pdf.split", None, now).is_none());
        assert!(default_output_name("unknown", None, now).is_none());
        assert!(default_output_name("pdf.compress", None, now).is_none());
    }

    #[test]
    fn default_names_have_exactly_one_pdf_extension() {
        let now = fixed_time();
        let names = [
            default_output_name("images.convert_to_pdf", None, now).unwrap(),
            default_output_name("pdf.merge", None, now).unwrap(),
            default_output_name("pdf.compress", Some(CompressionPreset::Balanced), now).unwrap(),
        ];

        for name in names {
            assert!(name.starts_with("Quicklet-"));
            assert!(name.ends_with(".pdf"));
            assert_eq!(name.matches(".pdf").count(), 1);
        }
    }

    #[test]
    fn builds_split_page_and_range_names() {
        assert_eq!(
            split_output_name("Report", 1, 1),
            "Quicklet-Split-Report-page-001.pdf"
        );
        assert_eq!(
            split_output_name("Report", 1, 3),
            "Quicklet-Split-Report-pages-001-003.pdf"
        );
        assert_eq!(
            split_output_name("Report", 12, 12),
            "Quicklet-Split-Report-page-012.pdf"
        );
        assert_eq!(
            split_output_name("Report", 999, 999),
            "Quicklet-Split-Report-page-999.pdf"
        );
        assert_eq!(
            split_output_name("Report", 1000, 1002),
            "Quicklet-Split-Report-pages-1000-1002.pdf"
        );
    }

    #[test]
    fn sanitizes_source_stems_and_removes_duplicate_pdf_extensions() {
        assert_eq!(
            split_output_name("  bad/name:2026?.PDF  ", 1, 1),
            "Quicklet-Split-bad-name-2026-page-001.pdf"
        );
        assert_eq!(
            split_output_name("Report.pdf", 1, 1),
            "Quicklet-Split-Report-page-001.pdf"
        );
        assert_eq!(
            split_output_name("Report.pdf.PDF", 1, 3),
            "Quicklet-Split-Report-pages-001-003.pdf"
        );
        assert_eq!(
            split_output_name("Résumé", 1, 1),
            "Quicklet-Split-Résumé-page-001.pdf"
        );
        assert_eq!(
            split_output_name("Report.pdf?", 1, 1),
            "Quicklet-Split-Report-page-001.pdf"
        );
        assert_eq!(
            split_output_name("Report.pdf.", 1, 1),
            "Quicklet-Split-Report-page-001.pdf"
        );
    }

    #[test]
    fn handles_empty_and_unsafe_stems() {
        for stem in ["", "   ", ".", "..", "...", ".pdf", "/", "\\", "???"] {
            assert_eq!(
                split_output_name(stem, 1, 1),
                "Quicklet-Split-Document-page-001.pdf",
                "stem {stem:?}"
            );
        }
    }

    #[test]
    fn truncates_very_long_source_stems_safely() {
        let stem = "x".repeat(500);
        let name = split_output_name(&stem, 1, 1);

        assert!(name.len() < 255);
        assert!(name.starts_with("Quicklet-Split-"));
        assert!(name.ends_with("-page-001.pdf"));
        assert_eq!(name.matches(".pdf").count(), 1);
    }
}
