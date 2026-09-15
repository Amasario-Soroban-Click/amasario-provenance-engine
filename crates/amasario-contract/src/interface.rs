//! Decoding a contract's declared interface from its specification section.
//!
//! # Where the interface comes from
//!
//! A Soroban contract built with the Soroban SDK writes a custom WebAssembly
//! section named `contractspecv0` containing a sequence of XDR `SCSpecEntry`
//! values: the contract's exported functions, the user-defined types those
//! functions use, and the events the contract may emit. This module reads that
//! section.
//!
//! # What the interface is, and is not
//!
//! The interface is a *declaration made by the module's builder*. It is evidence
//! about what the contract was compiled to provide, and it is the reason the
//! specification's dependency model treats interface similarity as a basis at all.
//! It is not a description of observed behaviour: a contract may be invoked through
//! any exported function, and the declaration says nothing about which invocations
//! have occurred. The engine therefore attaches an interface-derived dependency the
//! `INFERRED_INTERFACE` basis, which caps its confidence, rather than treating the
//! declaration as an observation.
//!
//! # Decoding failures
//!
//! The section is a concatenation of length-implied XDR values, so a truncated or
//! corrupted section fails part-way through. Two cases are distinguished:
//!
//! * Nothing decoded from a non-empty section: the section is unreadable, and the
//!   engine reports [`crate::errors::InspectionFailure::SpecSectionUndecodable`].
//! * Some entries decoded and then decoding stopped: the entries that were read are
//!   returned, and the number of unreadable trailing bytes is recorded. Reporting
//!   the whole interface as absent because its tail is damaged would discard
//!   evidence the engine actually has.
//!
//! Neither case affects the module's identity, which rests on its digest.

use std::io::Cursor;

use amasario_core::{Digest, EngineError, Result};
use serde::{Deserialize, Serialize};
use stellar_xdr::{
    Limited, Limits, ReadXdr, ScSpecEntry, ScSpecEventDataFormat, ScSpecEventV0, ScSpecFunctionV0,
    ScSpecTypeDef, ScSpecUdtEnumV0, ScSpecUdtUnionCaseV0, ScSpecUdtUnionV0, WriteXdr,
};

use crate::errors::InspectionFailure;

/// The prefix the Soroban SDK prepends to an event's name.
///
/// Recorded because a report that showed the raw name would show the internal
/// form rather than the name a contract author wrote.
const EVENT_NAME_PREFIX: &str = "STELLAR";

/// A parameter of an interface function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceParam {
    /// The parameter's name as declared.
    pub name: String,
    /// The parameter's declared type, in the specification's textual form.
    pub type_name: String,
    /// The declaration's documentation, when the author wrote any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

/// An exported function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceFunction {
    /// The exported name.
    pub name: String,
    /// The declared parameters, in declaration order.
    pub inputs: Vec<InterfaceParam>,
    /// The declared return type, in the specification's textual form.
    pub output: String,
    /// The declaration's documentation, when the author wrote any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

/// A field of a declared struct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceField {
    /// The field's name.
    pub name: String,
    /// The field's declared type.
    pub type_name: String,
}

/// A declared struct type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceStruct {
    /// The type's name.
    pub name: String,
    /// The library or contract the type was declared in, as reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lib: Option<String>,
    /// The fields, in declaration order.
    pub fields: Vec<InterfaceField>,
}

/// One case of a declared union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceUnionCase {
    /// The case's name.
    pub name: String,
    /// Whether the case carries no value (`void`) or a tuple.
    pub kind: String,
    /// The carried types, empty for a void case.
    pub type_names: Vec<String>,
}

/// A declared union type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceUnion {
    /// The type's name.
    pub name: String,
    /// The library or contract the type was declared in, as reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lib: Option<String>,
    /// The cases, in declaration order.
    pub cases: Vec<InterfaceUnionCase>,
}

/// One case of a declared enumeration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceEnumCase {
    /// The case's name.
    pub name: String,
    /// The case's numeric value, which is part of the contract's ABI.
    pub value: u32,
}

/// A declared enumeration, used for either an enum or an error enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceEnum {
    /// The type's name.
    pub name: String,
    /// The library or contract the type was declared in, as reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lib: Option<String>,
    /// The cases, in declaration order.
    pub cases: Vec<InterfaceEnumCase>,
}

/// A parameter of a declared event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceEventParam {
    /// The parameter's name.
    pub name: String,
    /// The parameter's declared type.
    pub type_name: String,
    /// Where the parameter appears: `topic` or `data`.
    pub location: String,
}

/// A declared event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceEvent {
    /// The event's name, without the SDK's prefix.
    pub name: String,
    /// The topics the SDK places before the declared ones.
    pub prefix_topics: Vec<String>,
    /// The event's parameters.
    pub params: Vec<InterfaceEventParam>,
    /// How the event's data is laid out.
    pub data_format: String,
    /// The declaration's documentation, when the author wrote any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

/// A contract's declared interface.
///
/// Each collection is sorted by name so that the interface's serialisation and
/// digest are independent of the order the entries happened to appear in the
/// section. A contract's declared order is not a fact about the contract, and
/// letting it leak into the digest would make two identical interfaces compare
/// unequal when their builders emitted entries differently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ContractInterface {
    /// The exported functions.
    pub functions: Vec<InterfaceFunction>,
    /// The declared struct types.
    pub structs: Vec<InterfaceStruct>,
    /// The declared union types.
    pub unions: Vec<InterfaceUnion>,
    /// The declared enum types.
    pub enums: Vec<InterfaceEnum>,
    /// The declared error-enum types.
    pub error_enums: Vec<InterfaceEnum>,
    /// The declared events.
    pub events: Vec<InterfaceEvent>,
    /// The number of trailing bytes that could not be decoded after at least one
    /// entry was read.
    pub undecodable_trailing_bytes: usize,
    /// A description of where decoding stopped, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decode_detail: Option<String>,
}

impl ContractInterface {
    /// Whether the interface declares nothing at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.functions.is_empty()
            && self.structs.is_empty()
            && self.unions.is_empty()
            && self.enums.is_empty()
            && self.error_enums.is_empty()
            && self.events.is_empty()
    }

    /// The declared function with this name, when there is one.
    ///
    /// Used to decide whether an observed invocation targets a name the contract
    /// declares, which is what separates an invocation of a declared entry point
    /// from an invocation of something else.
    #[must_use]
    pub fn function(&self, name: &str) -> Option<&InterfaceFunction> {
        self.functions.iter().find(|function| function.name == name)
    }

    /// The declared event with this name, when there is one.
    #[must_use]
    pub fn event(&self, name: &str) -> Option<&InterfaceEvent> {
        self.events.iter().find(|event| event.name == name)
    }

    /// A digest over the interface's canonical form.
    ///
    /// Separate from the module's digest and deliberately so: an upgrade can change
    /// a contract's executable without changing its interface, or change its
    /// interface without changing which functions an observer invokes. A consumer
    /// asking "did the interface change" needs an answer distinct from "did the
    /// module change", and deriving the first from the second would give the wrong
    /// answer in both cases.
    #[must_use]
    pub fn digest(&self) -> Digest {
        // `serde_json` with `preserve_order` is enabled workspace-wide, and the
        // collections are sorted here, so this serialisation is canonical. The
        // decode-failure fields are excluded: they describe the reading, not the
        // interface.
        let canonical = serde_json::json!({
            "functions": self.functions,
            "structs": self.structs,
            "unions": self.unions,
            "enums": self.enums,
            "errorEnums": self.error_enums,
            "events": self.events,
        });
        Digest::sha256_of(canonical.to_string().as_bytes())
    }

    /// Sorts every collection by name.
    ///
    /// Called at the end of decoding so that the sort cannot be forgotten, and so
    /// that [`ContractInterface::digest`] is order-independent.
    fn canonicalise(&mut self) {
        self.functions.sort_by(|a, b| a.name.cmp(&b.name));
        self.structs.sort_by(|a, b| a.name.cmp(&b.name));
        self.unions.sort_by(|a, b| a.name.cmp(&b.name));
        self.enums.sort_by(|a, b| a.name.cmp(&b.name));
        self.error_enums.sort_by(|a, b| a.name.cmp(&b.name));
        self.events.sort_by(|a, b| a.name.cmp(&b.name));
        for function in &mut self.functions {
            function.inputs.sort_by(|a, b| a.name.cmp(&b.name));
        }
    }
}

/// Decodes a contract specification section.
///
/// # Errors
///
/// Returns a contract error when the section is non-empty and no entry could be
/// decoded from it. A section that yields entries and then stops is *not* an error;
/// the entries are returned with the failure recorded, because discarding readable
/// evidence because its tail is damaged would lose information the engine holds.
pub fn decode_spec_section(section: &[u8]) -> Result<ContractInterface> {
    if section.is_empty() {
        // An empty payload is a contract with no declared interface, which is a
        // legitimate state rather than a decode failure.
        return Ok(ContractInterface::default());
    }

    // Entries are decoded one at a time from a moving offset rather than through the
    // framework's iterator, because the iterator cannot report how many bytes it
    // consumed - and how much of a damaged section was read is exactly the fact a
    // caller needs in order to know whether the interface it holds is complete.
    //
    // Each entry's length is recovered by re-encoding it. That is exact rather than
    // approximate: XDR has a single canonical encoding for every value, so a decoded
    // entry re-encodes to precisely the bytes it was decoded from. The test below
    // asserts the sum of the re-encoded lengths equals the section length for an
    // undamaged section, which is what makes the assumption checkable rather than
    // asserted.
    let mut interface = ContractInterface::default();
    let mut offset = 0_usize;
    let mut consumed: usize = 0;

    while offset < section.len() {
        let mut limited = Limited::new(Cursor::new(&section[offset..]), Limits::none());
        let entry = match ScSpecEntry::read_xdr(&mut limited) {
            Ok(entry) => entry,
            Err(error) => {
                if consumed == 0 {
                    return Err(InspectionFailure::SpecSectionUndecodable {
                        detail: format!(
                            "no entry could be decoded from the {} bytes at offset 0: {error}",
                            section.len().min(16)
                        ),
                    }
                    .into_error());
                }
                interface.decode_detail = Some(format!(
                    "decoding stopped at offset {offset} after {consumed} entries: {error}"
                ));
                break;
            },
        };

        let width = encoded_length(&entry)?;
        if width == 0 {
            // An entry always begins with a four-byte discriminant, so a zero width
            // would mean the measurement is wrong and the loop would never advance.
            // Reporting it beats spinning.
            return Err(InspectionFailure::SpecSectionUndecodable {
                detail: format!(
                    "entry {consumed} decoded but re-encoded to no bytes at all, which cannot 
                     happen for XDR"
                ),
            }
            .into_error());
        }

        offset += width;
        consumed += 1;
        interface.add_entry(&entry);
    }

    if interface.decode_detail.is_some() {
        interface.undecodable_trailing_bytes = section.len().saturating_sub(offset);
    }

    interface.canonicalise();
    Ok(interface)
}

/// The number of bytes an entry occupies in its canonical XDR encoding.
///
/// # Errors
///
/// Returns a contract error only if the entry cannot be encoded at all, which
/// cannot happen for a value that was just decoded.
fn encoded_length(entry: &ScSpecEntry) -> Result<usize> {
    let mut encoded: Vec<u8> = Vec::new();
    entry
        .write_xdr(&mut Limited::new(&mut encoded, Limits::none()))
        .map_err(|error| {
            InspectionFailure::SpecSectionUndecodable {
                detail: format!("a decoded entry could not be re-encoded to measure it: {error}"),
            }
            .into_error()
        })?;
    Ok(encoded.len())
}

impl ContractInterface {
    /// Records one decoded entry.
    fn add_entry(&mut self, entry: &ScSpecEntry) {
        match entry {
            ScSpecEntry::FunctionV0(function) => self.functions.push(function_of(function)),
            ScSpecEntry::UdtStructV0(structure) => self.structs.push(InterfaceStruct {
                name: structure.name.to_utf8_string_lossy(),
                lib: optional_text(&structure.lib),
                fields: structure
                    .fields
                    .iter()
                    .map(|field| InterfaceField {
                        name: field.name.to_utf8_string_lossy(),
                        type_name: type_name(&field.type_),
                    })
                    .collect(),
            }),
            ScSpecEntry::UdtUnionV0(union) => self.unions.push(union_of(union)),
            ScSpecEntry::UdtEnumV0(enumeration) => self.enums.push(enum_of(enumeration)),
            ScSpecEntry::UdtErrorEnumV0(enumeration) => {
                self.error_enums.push(InterfaceEnum {
                    name: enumeration.name.to_utf8_string_lossy(),
                    lib: optional_text(&enumeration.lib),
                    cases: enumeration
                        .cases
                        .iter()
                        .map(|case| InterfaceEnumCase {
                            name: case.name.to_utf8_string_lossy(),
                            value: case.value,
                        })
                        .collect(),
                });
            },
            ScSpecEntry::EventV0(event) => self.events.push(event_of(event)),
        }
    }
}

/// Converts a declared function.
fn function_of(function: &ScSpecFunctionV0) -> InterfaceFunction {
    // The outputs field is a sequence with a maximum of one element: a contract
    // function returns exactly one value, which may itself be a tuple or unit.
    let output = function
        .outputs
        .iter()
        .next()
        .map_or_else(|| "void".to_owned(), type_name);

    InterfaceFunction {
        name: function.name.0.to_utf8_string_lossy(),
        inputs: function
            .inputs
            .iter()
            .map(|input| InterfaceParam {
                name: input.name.to_utf8_string_lossy(),
                type_name: type_name(&input.type_),
                doc: optional_text(&input.doc),
            })
            .collect(),
        output,
        doc: optional_text(&function.doc),
    }
}

/// Converts a declared union.
fn union_of(union: &ScSpecUdtUnionV0) -> InterfaceUnion {
    InterfaceUnion {
        name: union.name.to_utf8_string_lossy(),
        lib: optional_text(&union.lib),
        cases: union
            .cases
            .iter()
            .map(|case| match case {
                ScSpecUdtUnionCaseV0::VoidV0(void) => InterfaceUnionCase {
                    name: void.name.to_utf8_string_lossy(),
                    kind: "VOID".to_owned(),
                    type_names: Vec::new(),
                },
                ScSpecUdtUnionCaseV0::TupleV0(tuple) => InterfaceUnionCase {
                    name: tuple.name.to_utf8_string_lossy(),
                    kind: "TUPLE".to_owned(),
                    type_names: tuple.type_.iter().map(type_name).collect(),
                },
            })
            .collect(),
    }
}

/// Converts a declared enumeration.
fn enum_of(enumeration: &ScSpecUdtEnumV0) -> InterfaceEnum {
    InterfaceEnum {
        name: enumeration.name.to_utf8_string_lossy(),
        lib: optional_text(&enumeration.lib),
        cases: enumeration
            .cases
            .iter()
            .map(|case| InterfaceEnumCase {
                name: case.name.to_utf8_string_lossy(),
                value: case.value,
            })
            .collect(),
    }
}

/// Converts a declared event, removing the SDK's prefix from its name.
fn event_of(event: &ScSpecEventV0) -> InterfaceEvent {
    let raw = event.name.0.to_utf8_string_lossy();
    // The Soroban SDK prefixes event names with the crate name, so the declared
    // name is the part after the first component. Reporting the raw form would show
    // an internal identifier where a report should show the author's name.
    let name = raw
        .split_once(EVENT_NAME_PREFIX)
        .map_or_else(|| raw.clone(), |(_, rest)| rest.to_owned());

    InterfaceEvent {
        name,
        prefix_topics: event
            .prefix_topics
            .iter()
            .map(|topic| topic.0.to_utf8_string_lossy())
            .collect(),
        params: event
            .params
            .iter()
            .map(|param| InterfaceEventParam {
                name: param.name.to_utf8_string_lossy(),
                type_name: type_name(&param.type_),
                location: match param.location {
                    stellar_xdr::ScSpecEventParamLocationV0::TopicList => "topic".to_owned(),
                    stellar_xdr::ScSpecEventParamLocationV0::Data => "data".to_owned(),
                },
            })
            .collect(),
        data_format: match event.data_format {
            ScSpecEventDataFormat::SingleValue => "SINGLE_VALUE".to_owned(),
            ScSpecEventDataFormat::Vec => "VEC".to_owned(),
            ScSpecEventDataFormat::Map => "MAP".to_owned(),
        },
        doc: optional_text(&event.doc),
    }
}

/// Renders a declared type in the specification's textual form.
///
/// The form is the one the contract-specification document defines, so that a
/// reader comparing this engine's output against the published interface sees the
/// same strings rather than an engine-specific rendering.
#[must_use]
pub fn type_name(declared: &ScSpecTypeDef) -> String {
    match declared {
        ScSpecTypeDef::Val => "val".to_owned(),
        ScSpecTypeDef::Bool => "bool".to_owned(),
        ScSpecTypeDef::Void => "void".to_owned(),
        ScSpecTypeDef::Error => "error".to_owned(),
        ScSpecTypeDef::U32 => "u32".to_owned(),
        ScSpecTypeDef::I32 => "i32".to_owned(),
        ScSpecTypeDef::U64 => "u64".to_owned(),
        ScSpecTypeDef::I64 => "i64".to_owned(),
        ScSpecTypeDef::Timepoint => "timepoint".to_owned(),
        ScSpecTypeDef::Duration => "duration".to_owned(),
        ScSpecTypeDef::U128 => "u128".to_owned(),
        ScSpecTypeDef::I128 => "i128".to_owned(),
        ScSpecTypeDef::U256 => "u256".to_owned(),
        ScSpecTypeDef::I256 => "i256".to_owned(),
        ScSpecTypeDef::Bytes => "bytes".to_owned(),
        ScSpecTypeDef::String => "string".to_owned(),
        ScSpecTypeDef::Symbol => "symbol".to_owned(),
        ScSpecTypeDef::Address => "address".to_owned(),
        ScSpecTypeDef::MuxedAddress => "muxed_address".to_owned(),
        ScSpecTypeDef::Option(option) => format!("option[{}]", type_name(&option.value_type)),
        ScSpecTypeDef::Result(result) => format!(
            "result[{},{}]",
            type_name(&result.ok_type),
            type_name(&result.error_type)
        ),
        ScSpecTypeDef::Vec(vector) => format!("vec[{}]", type_name(&vector.element_type)),
        ScSpecTypeDef::Map(map) => format!(
            "map[{},{}]",
            type_name(&map.key_type),
            type_name(&map.value_type)
        ),
        ScSpecTypeDef::Tuple(tuple) => {
            let inner: Vec<String> = tuple.value_types.iter().map(type_name).collect();
            format!("tuple[{}]", inner.join(","))
        },
        ScSpecTypeDef::BytesN(bytes) => format!("bytes[{}]", bytes.n),
        ScSpecTypeDef::Udt(udt) => udt.name.to_utf8_string_lossy(),
    }
}

/// Converts a length-limited string to text, treating an empty string as absent.
///
/// An empty documentation field and an absent one mean the same thing to a reader,
/// and a report that distinguished them would carry a field that is always
/// meaningless.
///
/// Generic over the field's bound because the specification's fields have different
/// bounds - a `lib` is at most 80 bytes and a `doc` at most 1024 - and a function
/// specialised to one bound would have to be duplicated for the other, which is how
/// the two copies come to differ.
fn optional_text<const N: u32>(value: &stellar_xdr::StringM<N>) -> Option<String> {
    let text = value.to_utf8_string_lossy();
    if text.is_empty() { None } else { Some(text) }
}

/// Builds the `SpecSectionUndecodable` failure for a section that is present but
/// unreadable.
///
/// Exposed so that a caller holding a module whose section failed to decode can
/// report the same failure the decoder produced rather than paraphrasing it.
#[must_use]
pub fn undecodable(detail: impl Into<String>) -> EngineError {
    InspectionFailure::SpecSectionUndecodable {
        detail: detail.into(),
    }
    .into_error()
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::{
        ScSpecEventParamLocationV0, ScSpecEventParamV0, ScSpecFunctionInputV0, ScSpecTypeBytesN,
        ScSpecTypeMap, ScSpecTypeOption, ScSpecTypeResult, ScSpecTypeTuple, ScSpecTypeUdt,
        ScSpecTypeVec, ScSpecUdtEnumCaseV0, ScSpecUdtEnumV0, ScSpecUdtErrorEnumCaseV0,
        ScSpecUdtErrorEnumV0, ScSpecUdtStructFieldV0, ScSpecUdtStructV0, ScSpecUdtUnionCaseTupleV0,
        ScSpecUdtUnionCaseVoidV0, ScSpecUdtUnionV0, ScSymbol, StringM, WriteXdr,
    };

    /// Encodes entries as the section the SDK writes: one XDR value after another.
    fn section(entries: &[ScSpecEntry]) -> Vec<u8> {
        let mut out = Vec::new();
        for entry in entries {
            entry
                .write_xdr(&mut Limited::new(&mut out, Limits::none()))
                .expect("a spec entry encodes");
        }
        out
    }

    fn text<const N: u32>(value: &str) -> StringM<N> {
        value.parse().expect("a short string")
    }

    fn symbol(value: &str) -> ScSymbol {
        // `ScSymbol` is a distinct generated type wrapping a bounded string rather
        // than an alias, so it has no `FromStr` of its own.
        ScSymbol(value.parse().expect("a short symbol"))
    }

    fn function(
        name: &str,
        inputs: &[(&str, ScSpecTypeDef)],
        output: ScSpecTypeDef,
    ) -> ScSpecEntry {
        ScSpecEntry::FunctionV0(ScSpecFunctionV0 {
            doc: text(""),
            name: symbol(name),
            inputs: inputs
                .iter()
                .map(|(name, type_)| ScSpecFunctionInputV0 {
                    doc: text(""),
                    name: text(name),
                    type_: type_.clone(),
                })
                .collect::<Vec<_>>()
                .try_into()
                .expect("a few inputs"),
            outputs: vec![output].try_into().expect("exactly one output"),
        })
    }

    fn udt(name: &str) -> ScSpecTypeDef {
        ScSpecTypeDef::Udt(ScSpecTypeUdt { name: text(name) })
    }

    fn structure(name: &str, fields: &[(&str, ScSpecTypeDef)]) -> ScSpecEntry {
        ScSpecEntry::UdtStructV0(ScSpecUdtStructV0 {
            doc: text(""),
            lib: text(""),
            name: text(name),
            fields: fields
                .iter()
                .map(|(name, type_)| ScSpecUdtStructFieldV0 {
                    doc: text(""),
                    name: text(name),
                    type_: type_.clone(),
                })
                .collect::<Vec<_>>()
                .try_into()
                .expect("a few fields"),
        })
    }

    #[test]
    fn an_empty_section_is_an_interface_that_declares_nothing() {
        // A contract with no declared interface is a legitimate state; the module's
        // identity is still established by its digest.
        let interface = decode_spec_section(&[]).expect("an empty section decodes");
        assert!(interface.is_empty());
        assert!(interface.decode_detail.is_none());
    }

    #[test]
    fn a_function_decodes_with_its_parameters_and_return_type() {
        let bytes = section(&[function(
            "transfer",
            &[
                ("from", ScSpecTypeDef::Address),
                ("amount", ScSpecTypeDef::I128),
            ],
            ScSpecTypeDef::Bool,
        )]);
        let interface = decode_spec_section(&bytes).expect("decodes");

        assert_eq!(interface.functions.len(), 1);
        let transfer = interface.function("transfer").expect("declared");
        assert_eq!(transfer.output, "bool");
        assert_eq!(transfer.inputs.len(), 2);
        // Inputs are sorted by name, so `amount` precedes `from`.
        assert_eq!(transfer.inputs[0].name, "amount");
        assert_eq!(transfer.inputs[1].type_name, "address");
    }

    #[test]
    fn a_function_with_no_declared_return_decodes_as_void() {
        // The output sequence has a maximum of one element and may be empty; a
        // missing rendering would put an empty string in a report.
        let bytes = section(&[function("ping", &[], ScSpecTypeDef::Void)]);
        let interface = decode_spec_section(&bytes).expect("decodes");
        assert_eq!(interface.function("ping").expect("declared").output, "void");

        let entries = &[ScSpecEntry::FunctionV0(ScSpecFunctionV0 {
            doc: text(""),
            name: symbol("bare"),
            inputs: Vec::new().try_into().expect("no inputs"),
            outputs: Vec::new().try_into().expect("no outputs"),
        })];
        let interface = decode_spec_section(&section(entries)).expect("decodes");
        assert_eq!(interface.function("bare").expect("declared").output, "void");
    }

    #[test]
    fn a_struct_decodes_with_its_fields() {
        let bytes = section(&[structure("Account", &[("id", ScSpecTypeDef::Address)])]);
        let interface = decode_spec_section(&bytes).expect("decodes");

        assert_eq!(interface.structs.len(), 1);
        assert_eq!(interface.structs[0].name, "Account");
        assert_eq!(interface.structs[0].fields[0].name, "id");
        assert_eq!(interface.structs[0].fields[0].type_name, "address");
        assert_eq!(interface.structs[0].lib, None, "an empty lib is absent");
    }

    #[test]
    fn an_enum_and_an_error_enum_are_kept_apart() {
        // They are different declarations: one is a data type, the other is the set
        // of errors a function may return, and merging them would lose which is
        // which.
        let enumeration = ScSpecEntry::UdtEnumV0(ScSpecUdtEnumV0 {
            doc: text(""),
            lib: text(""),
            name: text("Circuit"),
            cases: vec![ScSpecUdtEnumCaseV0 {
                doc: text(""),
                name: text("Closed"),
                value: 0,
            }]
            .try_into()
            .expect("one case"),
        });
        let failure = ScSpecEntry::UdtErrorEnumV0(ScSpecUdtErrorEnumV0 {
            doc: text(""),
            lib: text(""),
            name: text("Error"),
            cases: vec![ScSpecUdtErrorEnumCaseV0 {
                doc: text(""),
                name: text("NotFound"),
                value: 3,
            }]
            .try_into()
            .expect("one case"),
        });

        let interface = decode_spec_section(&section(&[enumeration, failure])).expect("decodes");
        assert_eq!(interface.enums.len(), 1);
        assert_eq!(interface.enums[0].cases[0].value, 0);
        assert_eq!(interface.error_enums.len(), 1);
        assert_eq!(interface.error_enums[0].cases[0].name, "NotFound");
        assert_eq!(interface.error_enums[0].cases[0].value, 3);
    }

    #[test]
    fn a_union_keeps_void_and_tuple_cases_distinct() {
        let union = ScSpecEntry::UdtUnionV0(ScSpecUdtUnionV0 {
            doc: text(""),
            lib: text(""),
            name: text("Shape"),
            cases: vec![
                ScSpecUdtUnionCaseV0::VoidV0(ScSpecUdtUnionCaseVoidV0 {
                    doc: text(""),
                    name: text("None"),
                }),
                ScSpecUdtUnionCaseV0::TupleV0(ScSpecUdtUnionCaseTupleV0 {
                    doc: text(""),
                    name: text("Pair"),
                    type_: vec![ScSpecTypeDef::U32, ScSpecTypeDef::U32]
                        .try_into()
                        .expect("two types"),
                }),
            ]
            .try_into()
            .expect("two cases"),
        });

        let interface = decode_spec_section(&section(&[union])).expect("decodes");
        assert_eq!(interface.unions[0].cases[0].kind, "VOID");
        assert!(interface.unions[0].cases[0].type_names.is_empty());
        assert_eq!(interface.unions[0].cases[1].kind, "TUPLE");
        assert_eq!(interface.unions[0].cases[1].type_names, ["u32", "u32"]);
    }

    #[test]
    fn composite_type_names_are_rendered_in_the_published_form() {
        assert_eq!(
            type_name(&ScSpecTypeDef::Option(Box::new(ScSpecTypeOption {
                value_type: Box::new(ScSpecTypeDef::Address),
            }))),
            "option[address]"
        );
        assert_eq!(
            type_name(&ScSpecTypeDef::Vec(Box::new(ScSpecTypeVec {
                element_type: Box::new(ScSpecTypeDef::U32),
            }))),
            "vec[u32]"
        );
        assert_eq!(
            type_name(&ScSpecTypeDef::Map(Box::new(ScSpecTypeMap {
                key_type: Box::new(ScSpecTypeDef::Symbol),
                value_type: Box::new(ScSpecTypeDef::I128),
            }))),
            "map[symbol,i128]"
        );
        assert_eq!(
            type_name(&ScSpecTypeDef::Tuple(Box::new(ScSpecTypeTuple {
                value_types: vec![ScSpecTypeDef::U32, ScSpecTypeDef::U64]
                    .try_into()
                    .expect("two types"),
            }))),
            "tuple[u32,u64]"
        );
        assert_eq!(
            type_name(&ScSpecTypeDef::Result(Box::new(ScSpecTypeResult {
                ok_type: Box::new(ScSpecTypeDef::U32),
                error_type: Box::new(ScSpecTypeDef::Error),
            }))),
            "result[u32,error]"
        );
        assert_eq!(
            type_name(&ScSpecTypeDef::BytesN(ScSpecTypeBytesN { n: 32 })),
            "bytes[32]"
        );
        assert_eq!(type_name(&udt("Account")), "Account");
        assert_eq!(type_name(&ScSpecTypeDef::MuxedAddress), "muxed_address");
    }

    #[test]
    fn the_sdk_prefix_is_removed_from_a_declared_events_name() {
        // Showing the raw form would put an internal identifier where a report
        // should show the name the author wrote.
        let event = ScSpecEntry::EventV0(ScSpecEventV0 {
            doc: text(""),
            lib: text(""),
            name: symbol("STELLARtransfer"),
            prefix_topics: vec![symbol("STELLAR")].try_into().expect("one topic"),
            params: vec![ScSpecEventParamV0 {
                doc: text(""),
                name: text("amount"),
                type_: ScSpecTypeDef::I128,
                location: ScSpecEventParamLocationV0::Data,
            }]
            .try_into()
            .expect("one param"),
            data_format: ScSpecEventDataFormat::Map,
        });

        let interface = decode_spec_section(&section(&[event])).expect("decodes");
        let declared = interface.event("transfer").expect("declared");
        assert_eq!(declared.params[0].location, "data");
        assert_eq!(declared.data_format, "MAP");
        assert_eq!(declared.prefix_topics, ["STELLAR"]);
    }

    #[test]
    fn the_interface_digest_is_independent_of_declaration_order() {
        // A contract's declared order is not a fact about the contract, so it must
        // not leak into an identity.
        let forward = section(&[
            function("alpha", &[], ScSpecTypeDef::Void),
            function("beta", &[], ScSpecTypeDef::Void),
        ]);
        let backward = section(&[
            function("beta", &[], ScSpecTypeDef::Void),
            function("alpha", &[], ScSpecTypeDef::Void),
        ]);

        let first = decode_spec_section(&forward).expect("decodes");
        let second = decode_spec_section(&backward).expect("decodes");
        assert_eq!(first.functions[0].name, "alpha");
        assert_eq!(second.functions[0].name, "alpha");
        assert_eq!(first.digest(), second.digest());
    }

    #[test]
    fn a_changed_interface_produces_a_different_digest() {
        let before = decode_spec_section(&section(&[function(
            "transfer",
            &[("amount", ScSpecTypeDef::I128)],
            ScSpecTypeDef::Void,
        )]))
        .expect("decodes");
        let after = decode_spec_section(&section(&[function(
            "transfer",
            &[
                ("amount", ScSpecTypeDef::I128),
                ("memo", ScSpecTypeDef::Symbol),
            ],
            ScSpecTypeDef::Void,
        )]))
        .expect("decodes");

        assert_ne!(
            before.digest(),
            after.digest(),
            "an added parameter changes the interface"
        );
        assert_eq!(
            before.function("transfer").expect("declared").output,
            after.function("transfer").expect("declared").output,
            "the return type is unchanged, which is why the digest must be the discriminator"
        );
    }

    #[test]
    fn a_section_whose_first_entry_is_unreadable_is_reported_as_undecodable() {
        let error = decode_spec_section(&[0xff, 0xff, 0xff])
            .expect_err("the first value cannot be decoded");
        let message = error.to_string();
        assert!(message.contains("could not be decoded"), "got: {message}");
        assert!(
            message.contains("remains verifiable by digest"),
            "the failure must not deny the verification that still holds: {message}"
        );
    }

    #[test]
    fn a_section_that_decodes_partially_keeps_what_it_read() {
        // Discarding readable evidence because its tail is damaged would lose
        // information the engine actually holds.
        let mut bytes = section(&[function("alpha", &[], ScSpecTypeDef::Void)]);
        let decoded_len = bytes.len();
        bytes.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);

        let interface = decode_spec_section(&bytes).expect("a partial section is not a failure");
        assert_eq!(interface.functions.len(), 1);
        assert!(interface.decode_detail.is_some(), "the damage is recorded");
        assert_eq!(
            interface.undecodable_trailing_bytes,
            bytes.len() - decoded_len,
            "how much of the section was unreadable must be exact, not an estimate"
        );
    }

    #[test]
    fn re_encoded_entry_lengths_account_for_the_whole_section() {
        // The decoder recovers each entry's width by re-encoding it, which is only
        // sound because XDR has exactly one canonical encoding per value. This
        // asserts that assumption instead of trusting it.
        let entries = [
            function("a", &[("x", ScSpecTypeDef::U32)], ScSpecTypeDef::Void),
            structure("S", &[("y", ScSpecTypeDef::I128)]),
        ];
        let bytes = section(&entries);
        let measured: usize = entries
            .iter()
            .map(|entry| encoded_length(entry).expect("an entry encodes"))
            .sum();

        assert_eq!(
            measured,
            bytes.len(),
            "the sum of the re-encoded widths must be the section length"
        );
        // And an undamaged section therefore has nothing trailing.
        let interface = decode_spec_section(&bytes).expect("decodes");
        assert_eq!(interface.undecodable_trailing_bytes, 0);
    }

    #[test]
    fn a_longer_interface_decodes_every_entry_it_was_given() {
        // A section is a concatenation, not a single value: a decoder that read one
        // entry and stopped would pass the single-entry tests and fail here.
        let entries: Vec<ScSpecEntry> = (0..64)
            .map(|index| function(&format!("f{index}"), &[], ScSpecTypeDef::Void))
            .collect();
        let interface = decode_spec_section(&section(&entries)).expect("decodes");

        assert_eq!(interface.functions.len(), 64);
        assert!(interface.decode_detail.is_none());
        for index in 0..64 {
            interface
                .function(&format!("f{index}"))
                .unwrap_or_else(|| panic!("f{index} must be declared"));
        }
    }

    #[test]
    fn an_undeclared_name_is_absent_rather_than_defaulted() {
        let interface =
            decode_spec_section(&section(&[function("alpha", &[], ScSpecTypeDef::Void)]))
                .expect("decodes");
        assert!(interface.function("not_declared").is_none());
        assert!(interface.event("not_declared").is_none());
    }

    #[test]
    fn decoding_is_deterministic_across_repeated_calls() {
        let bytes = section(&[
            function("alpha", &[("x", ScSpecTypeDef::U32)], ScSpecTypeDef::Void),
            structure("Record", &[("y", udt("Record"))]),
        ]);
        let first = decode_spec_section(&bytes).expect("decodes");
        for _ in 0..16 {
            assert_eq!(decode_spec_section(&bytes).expect("decodes"), first);
        }
        assert_eq!(
            serde_json::to_string(&first).expect("serialises"),
            serde_json::to_string(&decode_spec_section(&bytes).expect("decodes"))
                .expect("serialises")
        );
    }
}
