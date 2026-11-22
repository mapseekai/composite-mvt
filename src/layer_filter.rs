use std::collections::{HashMap, hash_map::Entry};

use buffa::encoding::{Tag, WireType, check_wire_type, decode_varint, skip_field};
use buffa::types::{borrow_bytes, borrow_str};
use buffa::{DecodeError, HasMessageView};
use fast_mvt::proto::Value;

use crate::{ComposeError, MvtSource, SourceError};

#[derive(Clone, Copy)]
pub(crate) enum Precedence {
    First,
    Last,
}

pub(crate) struct LayerFilter {
    pub(crate) precedence: Precedence,
    pub(crate) scan_sources: Box<[bool]>,
}

impl LayerFilter {
    pub(crate) fn compose<'a>(
        &self,
        sources: &[MvtSource],
        inputs: impl Iterator<Item = (usize, &'a [u8])>,
    ) -> Result<Vec<u8>, ComposeError> {
        let mut winners = HashMap::new();
        let mut fields = Vec::new();

        for (index, input) in inputs {
            if !self.scan_sources[index] {
                fields.push(Some(input));
                continue;
            }

            let mut remaining = input;
            while !remaining.is_empty() {
                let before = remaining;
                let name = tile_field_name(&mut remaining).map_err(|source| {
                    ComposeError::SourceParsing {
                        source_id: sources[index].id().clone(),
                        source,
                    }
                })?;

                if let Some(name) = name {
                    match winners.entry(name) {
                        Entry::Vacant(entry) => {
                            entry.insert(fields.len());
                        }
                        Entry::Occupied(_) if matches!(self.precedence, Precedence::First) => {
                            continue;
                        }
                        Entry::Occupied(mut entry) => {
                            let previous = entry.insert(fields.len());
                            fields[previous] = None;
                        }
                    }
                }

                fields.push(Some(&before[..before.len() - remaining.len()]));
            }
        }

        let mut output = Vec::with_capacity(fields.iter().flatten().map(|field| field.len()).sum());
        for field in fields.into_iter().flatten() {
            output.extend_from_slice(field);
        }
        Ok(output)
    }
}

fn tile_field_name<'a>(input: &mut &'a [u8]) -> Result<Option<&'a str>, SourceError> {
    let tag = Tag::decode(input).map_err(|_| SourceError::InvalidMvt)?;
    if tag.field_number() != 3 {
        skip_field(tag, input).map_err(|_| SourceError::InvalidMvt)?;
        return Ok(None);
    }
    match message_bytes(tag, input).and_then(layer_name) {
        Ok("") => Err(SourceError::MissingLayerName),
        Ok(name) => Ok(Some(name)),
        Err(_) => Err(SourceError::InvalidMvt),
    }
}

fn message_bytes<'a>(tag: Tag, input: &mut &'a [u8]) -> Result<&'a [u8], DecodeError> {
    check_wire_type(tag, WireType::LengthDelimited)?;
    borrow_bytes(input)
}

fn layer_name(mut input: &[u8]) -> Result<&str, DecodeError> {
    let mut name = "";
    while !input.is_empty() {
        let tag = Tag::decode(&mut input)?;
        match tag.field_number() {
            1 | 3 => {
                check_wire_type(tag, WireType::LengthDelimited)?;
                let text = borrow_str(&mut input)?;
                if tag.field_number() == 1 {
                    name = text;
                }
            }
            2 => scan_feature(message_bytes(tag, &mut input)?)?,
            4 => {
                Value::decode_view(message_bytes(tag, &mut input)?)?;
            }
            5 | 15 => {
                check_wire_type(tag, WireType::Varint)?;
                decode_varint(&mut input)?;
            }
            _ => skip_field(tag, &mut input)?,
        }
    }
    Ok(name)
}

fn scan_feature(mut input: &[u8]) -> Result<(), DecodeError> {
    while !input.is_empty() {
        let tag = Tag::decode(&mut input)?;
        match tag.field_number() {
            2 | 4 if tag.wire_type() == WireType::LengthDelimited => {
                let mut packed = borrow_bytes(&mut input)?;
                while !packed.is_empty() {
                    decode_varint(&mut packed)?;
                }
            }
            1..=4 => {
                check_wire_type(tag, WireType::Varint)?;
                decode_varint(&mut input)?;
            }
            _ => skip_field(tag, &mut input)?,
        }
    }
    Ok(())
}
