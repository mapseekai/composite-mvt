use std::collections::HashMap;
use std::collections::hash_map::Entry;

use buffa::DecodeError;
use buffa::encoding::{self, Tag, WireType};
use buffa::types;
use fast_mvt::proto::Value;

use crate::error::{ComposeErrorKind, SourceErrorKind};
use crate::{ComposeError, MvtSource, SourceError};

// MVT 2.1 field numbers follow the official Protobuf schema.
// https://github.com/mapbox/vector-tile-spec/blob/master/2.1/vector_tile.proto
const TILE_LAYERS: u32 = 3;

const LAYER_NAME: u32 = 1;
const LAYER_FEATURES: u32 = 2;
const LAYER_KEYS: u32 = 3;
const LAYER_VALUES: u32 = 4;
const LAYER_EXTENT: u32 = 5;
const LAYER_VERSION: u32 = 15;

const FEATURE_ID: u32 = 1;
const FEATURE_TAGS: u32 = 2;
const FEATURE_TYPE: u32 = 3;
const FEATURE_GEOMETRY: u32 = 4;

#[derive(Debug, Clone, Copy)]
pub(crate) enum Precedence {
    First,
    Last,
}

#[derive(Debug)]
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
                    ComposeErrorKind::SourceParsing {
                        source_id: sources[index].id().clone(),
                        cause: Box::new(source),
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

        let output_len = fields.iter().flatten().map(|field| field.len()).sum();
        ComposeError::check_output_len(output_len)?;
        let mut output = Vec::with_capacity(output_len);
        for field in fields.into_iter().flatten() {
            output.extend_from_slice(field);
        }
        Ok(output)
    }
}

fn tile_field_name<'a>(input: &mut &'a [u8]) -> Result<Option<&'a str>, SourceError> {
    let tag = Tag::decode(input).map_err(SourceError::invalid_mvt)?;
    if tag.field_number() != TILE_LAYERS {
        encoding::skip_field(tag, input).map_err(SourceError::invalid_mvt)?;
        return Ok(None);
    }
    match message_bytes(tag, input).and_then(layer_name) {
        Ok("") => Err(SourceErrorKind::MissingLayerName.into()),
        Ok(name) => Ok(Some(name)),
        Err(error) => Err(SourceError::invalid_mvt(error)),
    }
}

fn message_bytes<'a>(tag: Tag, input: &mut &'a [u8]) -> Result<&'a [u8], DecodeError> {
    encoding::check_wire_type(tag, WireType::LengthDelimited)?;
    types::borrow_bytes(input)
}

fn layer_name(mut input: &[u8]) -> Result<&str, DecodeError> {
    use buffa::HasMessageView as _;

    let mut name = "";
    while !input.is_empty() {
        let tag = Tag::decode(&mut input)?;
        match tag.field_number() {
            LAYER_NAME | LAYER_KEYS => {
                encoding::check_wire_type(tag, WireType::LengthDelimited)?;
                let text = types::borrow_str(&mut input)?;
                if tag.field_number() == LAYER_NAME {
                    name = text;
                }
            }
            LAYER_FEATURES => scan_feature(message_bytes(tag, &mut input)?)?,
            LAYER_VALUES => {
                Value::decode_view(message_bytes(tag, &mut input)?)?;
            }
            LAYER_EXTENT | LAYER_VERSION => {
                encoding::check_wire_type(tag, WireType::Varint)?;
                encoding::decode_varint(&mut input)?;
            }
            _ => encoding::skip_field(tag, &mut input)?,
        }
    }
    Ok(name)
}

fn scan_feature(mut input: &[u8]) -> Result<(), DecodeError> {
    while !input.is_empty() {
        let tag = Tag::decode(&mut input)?;
        match tag.field_number() {
            FEATURE_TAGS | FEATURE_GEOMETRY if tag.wire_type() == WireType::LengthDelimited => {
                let mut packed = types::borrow_bytes(&mut input)?;
                while !packed.is_empty() {
                    encoding::decode_varint(&mut packed)?;
                }
            }
            FEATURE_ID | FEATURE_TAGS | FEATURE_TYPE | FEATURE_GEOMETRY => {
                encoding::check_wire_type(tag, WireType::Varint)?;
                encoding::decode_varint(&mut input)?;
            }
            _ => encoding::skip_field(tag, &mut input)?,
        }
    }
    Ok(())
}
