//! The chunk PostProcessing NBT field, without writing or finalizing a world.

use anyhow::{Context, Result, bail, ensure};
use native_core::postprocessing::PostProcessing;

use crate::nbt::{Tag, TagType};

/// Encode Java's list of per-section short lists, preserving order and duplicates.
pub fn encode(marks: &PostProcessing) -> Tag {
    Tag::List {
        element_type: TagType::List,
        values: marks
            .sections()
            .iter()
            .map(|values| Tag::List {
                element_type: if values.is_empty() {
                    TagType::End
                } else {
                    TagType::Short
                },
                values: values.iter().copied().map(Tag::Short).collect(),
            })
            .collect(),
    }
}

/// Decode the field without coercing or discarding unknown content. A wrong
/// section count or unsupported NBT type is rejected for safe round trips.
pub fn decode(tag: &Tag, min_y: i32, height: i32) -> Result<PostProcessing> {
    let Tag::List {
        element_type: TagType::List,
        values,
    } = tag
    else {
        bail!("postprocessing must be a list of section lists");
    };
    let expected = PostProcessing::new(min_y, height).context("invalid postprocessing bounds")?;
    ensure!(
        values.len() == expected.sections().len(),
        "postprocessing section count differs from dimension"
    );
    let mut sections = Vec::with_capacity(values.len());
    for value in values {
        let Tag::List {
            element_type,
            values,
        } = value
        else {
            bail!("invalid postprocessing section");
        };
        ensure!(
            *element_type == TagType::Short || (*element_type == TagType::End && values.is_empty()),
            "postprocessing section must contain shorts"
        );
        let mut section = Vec::with_capacity(values.len());
        for value in values {
            let Tag::Short(value) = value else {
                bail!("invalid postprocessing offset");
            };
            section.push(*value);
        }
        sections.push(section);
    }
    PostProcessing::from_sections(min_y, height, sections)
        .context("invalid postprocessing sections")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nbt_preserves_raw_values_and_rejects_lossy_input() {
        let sections = vec![
            vec![i16::MIN, -1, 0, 0xf0f, 0xf0f],
            vec![],
            vec![i16::MAX, 255],
            vec![],
        ];
        let marks = PostProcessing::from_sections(-32, 64, sections.clone()).unwrap();
        let tag = encode(&marks);
        assert_eq!(decode(&tag, -32, 64).unwrap().sections(), sections);
        assert!(decode(&tag, -32, 48).is_err());
        assert!(decode(&Tag::Int(0), -32, 64).is_err());
        let mut bad = tag.clone();
        if let Tag::List { values, .. } = &mut bad {
            values[0] = Tag::List {
                element_type: TagType::Int,
                values: vec![Tag::Int(2)],
            };
        }
        assert!(decode(&bad, -32, 64).is_err());
        let mut bad = tag;
        if let Tag::List { values, .. } = &mut bad {
            values[0] = Tag::List {
                element_type: TagType::Short,
                values: vec![Tag::Int(2)],
            };
        }
        assert!(decode(&bad, -32, 64).is_err());
    }
}
