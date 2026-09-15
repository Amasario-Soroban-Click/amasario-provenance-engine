//! WebAssembly module inspection.
//!
//! # What this module actually does
//!
//! It parses the module's section framing, extracts the imported and exported
//! names, and locates the custom section Soroban uses to publish a contract's
//! interface. It does **not** validate the module against the WebAssembly
//! specification, execute it, or interpret its code section. Those are different
//! jobs with different failure modes, and claiming to do them would be the kind of
//! overstatement this engine exists to avoid: the engine's question is *where did
//! this module come from*, not *is this module well-formed enough to run*.
//!
//! The one thing it does decide is identity, and it decides it strictly:
//! [`WasmModule::verify_deployed_digest`] recomputes the SHA-256 of the bytes and
//! compares it against the digest the network records. A mismatch is a
//! contradiction, not a failed check - the engine is holding two facts that cannot
//! both be true, and the specification requires that case to be representable
//! rather than rounded to a pass or a fail.
//!
//! # Why the framing is parsed at all
//!
//! The interface section is a *custom* section, which the WebAssembly format
//! length-prefixes and allows anywhere. Finding it therefore requires walking the
//! framing. Doing that also gives the imports and exports for free, and those are
//! evidence: an import naming a host function is an observation about what the
//! module's code was compiled against, and it is the basis for the `WASM`
//! dependency class.
//!
//! # Strictness
//!
//! Two properties are checked beyond framing. A non-custom section may not appear
//! twice, and a section whose length overruns the module is rejected. Neither can
//! be produced by a conforming encoder, so both indicate corruption rather than an
//! unusual-but-valid module. Section *ordering* is deliberately not checked: the
//! specification requires the data-count section to precede the code section even
//! though its identifier is higher, and a monotonicity check would therefore reject
//! valid modules.

use std::fmt;

use amasario_core::{Digest, DigestAlgorithm, EngineError, Result};
use serde::{Deserialize, Serialize};

use crate::errors::InspectionFailure;

/// The WebAssembly module magic number, `\0asm`.
pub const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6d];

/// The only binary format version Soroban targets.
pub const WASM_VERSION: [u8; 4] = [0x01, 0x00, 0x00, 0x00];

/// The name of the custom section that carries a contract's declared interface.
///
/// A protocol fact, taken from the Soroban contract-specification format rather
/// than chosen here: `contractspecv0` is the name the Soroban SDK's build step
/// emits and the name the host reads.
pub const CONTRACT_SPEC_SECTION: &str = "contractspecv0";

/// The name of the custom section that carries the environment metadata.
///
/// Present in modules built by the Soroban SDK. Recorded when found because it
/// names the interface version the module was compiled against, which is part of
/// build provenance.
pub const CONTRACT_ENV_META_SECTION: &str = "contractenvmetav0";

/// The highest section identifier this parser recognises.
///
/// Section identifiers 0 through 12 are defined by the WebAssembly core
/// specification plus the exception-handling proposal's tag section. A higher
/// identifier cannot be skipped safely in a tool whose job is to decide identity:
/// silently ignoring a section it does not understand would mean attesting to a
/// module while having read only part of it.
pub const MAX_KNOWN_SECTION_ID: u8 = 13;

/// An upper bound on the number of names decoded from one section.
///
/// Not a protocol limit. The counts in the framing are attacker-controlled values
/// decoded from a byte string, so an unbounded loop driven by one could allocate
/// without limit. A module with more imports or exports than this is not a Soroban
/// contract module, so refusing is both safe and accurate.
pub const MAX_DECODED_ENTRIES: u32 = 65_536;

/// The identifier of a WebAssembly section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SectionKind {
    /// A custom section, whose name is the first field of its payload.
    Custom,
    /// Function signatures.
    Type,
    /// Imported functions, tables, memories, globals and tags.
    Import,
    /// The type index of each function defined in this module.
    Function,
    /// Tables.
    Table,
    /// Memories.
    Memory,
    /// Globals.
    Global,
    /// Exported names.
    Export,
    /// The function index that runs at instantiation.
    Start,
    /// Element segments.
    Element,
    /// Function bodies.
    Code,
    /// Data segments.
    Data,
    /// The number of data segments, placed before the code section.
    DataCount,
    /// Exception tags.
    Tag,
}

impl SectionKind {
    /// The section's identifier in the binary format.
    #[must_use]
    pub const fn id(self) -> u8 {
        match self {
            Self::Custom => 0,
            Self::Type => 1,
            Self::Import => 2,
            Self::Function => 3,
            Self::Table => 4,
            Self::Memory => 5,
            Self::Global => 6,
            Self::Export => 7,
            Self::Start => 8,
            Self::Element => 9,
            Self::Code => 10,
            Self::Data => 11,
            Self::DataCount => 12,
            Self::Tag => 13,
        }
    }

    /// The kind a section identifier denotes, when it is one this parser knows.
    #[must_use]
    pub const fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Custom),
            1 => Some(Self::Type),
            2 => Some(Self::Import),
            3 => Some(Self::Function),
            4 => Some(Self::Table),
            5 => Some(Self::Memory),
            6 => Some(Self::Global),
            7 => Some(Self::Export),
            8 => Some(Self::Start),
            9 => Some(Self::Element),
            10 => Some(Self::Code),
            11 => Some(Self::Data),
            12 => Some(Self::DataCount),
            13 => Some(Self::Tag),
            _ => None,
        }
    }

    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::Type => "type",
            Self::Import => "import",
            Self::Function => "function",
            Self::Table => "table",
            Self::Memory => "memory",
            Self::Global => "global",
            Self::Export => "export",
            Self::Start => "start",
            Self::Element => "element",
            Self::Code => "code",
            Self::Data => "data",
            Self::DataCount => "data_count",
            Self::Tag => "tag",
        }
    }
}

impl fmt::Display for SectionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One section as located in the framing.
///
/// Offsets are recorded so that a reader can locate a section in the module they
/// hold, which is what makes this usable for a diagnostic rather than only for a
/// summary. They are byte offsets from the start of the module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WasmSection {
    /// Which section this is.
    pub kind: SectionKind,
    /// The section's name, for a custom section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The offset of the section's identifier byte.
    pub offset: usize,
    /// The length of the section's payload, excluding its identifier and length.
    pub payload_len: usize,
}

/// What kind of item an import brings in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ImportKind {
    /// A function, carrying its type index.
    Function(u32),
    /// A table.
    Table,
    /// A memory.
    Memory,
    /// A global.
    Global,
    /// An exception tag.
    Tag,
}

impl ImportKind {
    /// The stable kind name, without the function's type index.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Function(_) => "FUNCTION",
            Self::Table => "TABLE",
            Self::Memory => "MEMORY",
            Self::Global => "GLOBAL",
            Self::Tag => "TAG",
        }
    }
}

/// A named import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WasmImport {
    /// The importing namespace, `env` for Soroban host functions.
    pub module: String,
    /// The imported name.
    pub field: String,
    /// What kind of item is imported.
    pub kind: ImportKind,
}

/// A named export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WasmExport {
    /// The exported name.
    pub name: String,
    /// The exporter kind: function, table, memory or global.
    pub kind: String,
}

/// A deployed module, as observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WasmModule {
    /// The SHA-256 digest of the module's bytes as retrieved.
    ///
    /// Always the digest of *these* bytes. Whether it equals the digest the network
    /// records is a separate question answered by
    /// [`WasmModule::verify_deployed_digest`], so that a module retrieved from a
    /// file and a module retrieved from a contract go through one comparison rather
    /// than two.
    pub module_digest: Digest,
    /// The module's size in bytes.
    pub size: usize,
    /// The sections found, in the order they appear.
    pub sections: Vec<WasmSection>,
    /// The imported names.
    pub imports: Vec<WasmImport>,
    /// The exported names.
    pub exports: Vec<WasmExport>,
    /// The contract specification section's payload, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_section: Option<Vec<u8>>,
    /// The environment metadata section's payload, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_meta_section: Option<Vec<u8>>,
}

impl WasmModule {
    /// Whether the module declares a contract interface.
    ///
    /// A module without the section is not necessarily incomplete - the Soroban SDK
    /// writes it during the contract's build, and a module built without that step
    /// has no interface to read. Reporting that as an error would be wrong; the
    /// engine reports the interface as absent and continues, because the module's
    /// identity is still established by its digest.
    #[must_use]
    pub const fn declares_interface(&self) -> bool {
        self.spec_section.is_some()
    }

    /// Whether the module's bytes hash to the digest the network records for it.
    ///
    /// # Errors
    ///
    /// Returns a provenance error stating both digests when they differ. The two
    /// values are reported rather than reduced to a boolean because the caller needs
    /// to show them.
    pub fn verify_deployed_digest(&self, recorded: &Digest) -> Result<()> {
        if self.module_digest.matches(recorded) {
            return Ok(());
        }
        Err(InspectionFailure::DigestContradiction {
            contract_id: String::new(),
            recorded: recorded.value().to_owned(),
            computed: self.module_digest.value().to_owned(),
        }
        .into_error())
    }

    /// The names of the host functions the module imports from `env`.
    ///
    /// Only `env` imports are returned, and this is a protocol fact rather than a
    /// convenience: Soroban host functions live in the `env` namespace, so an import
    /// from another namespace is a build artefact of the toolchain rather than a
    /// statement about the host interface. Returning them all and letting a caller
    /// filter would invite the filter to be forgotten.
    #[must_use]
    pub fn host_function_imports(&self) -> Vec<&WasmImport> {
        self.imports
            .iter()
            .filter(|import| {
                import.module == "env" && matches!(import.kind, ImportKind::Function(_))
            })
            .collect()
    }

    /// The sections of one kind.
    #[must_use]
    pub fn sections_of(&self, kind: SectionKind) -> Vec<&WasmSection> {
        self.sections
            .iter()
            .filter(|section| section.kind == kind)
            .collect()
    }
}

/// Parses a module's framing, imports and exports.
///
/// # Errors
///
/// Returns a contract error when the bytes are not a WebAssembly module of the
/// supported version, when a section's length overruns the module, when a
/// non-custom section appears twice, when a count exceeds
/// [`MAX_DECODED_ENTRIES`], or when a section identifier is above
/// [`MAX_KNOWN_SECTION_ID`].
pub fn parse_module(bytes: &[u8]) -> Result<WasmModule> {
    let mut cursor = Cursor::new(bytes);

    let magic = cursor.take(4)?;
    if magic != WASM_MAGIC {
        return Err(malformed(format!(
            "the first four bytes are {}, but a module begins with the magic number 00 61 73 6d",
            hex::encode(magic)
        )));
    }
    let version = cursor.take(4)?;
    if version != WASM_VERSION {
        return Err(malformed(format!(
            "binary format version is {}, but this engine reads version 1 (01 00 00 00)",
            hex::encode(version)
        )));
    }

    let mut sections: Vec<WasmSection> = Vec::new();
    let mut imports: Vec<WasmImport> = Vec::new();
    let mut exports: Vec<WasmExport> = Vec::new();
    let mut spec_section: Option<Vec<u8>> = None;
    let mut env_meta_section: Option<Vec<u8>> = None;

    while cursor.has_remaining() {
        let offset = cursor.position();
        let id = cursor.u8()?;
        let kind = SectionKind::from_id(id).ok_or_else(|| {
            malformed(format!(
                "section identifier {id} at offset {offset} is above {MAX_KNOWN_SECTION_ID}, which \
                 is the highest this engine recognises; skipping it would mean attesting to a \
                 module that was only partly read"
            ))
        })?;
        let payload_len = cursor.u32_leb()? as usize;
        let payload = cursor.take(payload_len)?.to_vec();

        // A non-custom section may appear at most once. Two occurrences cannot come
        // from a conforming encoder, so this indicates corruption rather than an
        // unusual module.
        if kind != SectionKind::Custom && sections.iter().any(|existing| existing.kind == kind) {
            return Err(malformed(format!(
                "the {kind} section appears more than once, which a conforming encoder cannot emit"
            )));
        }

        let name = if kind == SectionKind::Custom {
            let mut inner = Cursor::new(&payload);
            let name = inner.name()?;
            if name == CONTRACT_SPEC_SECTION && spec_section.is_none() {
                spec_section = Some(inner.rest().to_vec());
            } else if name == CONTRACT_ENV_META_SECTION && env_meta_section.is_none() {
                env_meta_section = Some(inner.rest().to_vec());
            }
            Some(name)
        } else {
            None
        };

        match kind {
            SectionKind::Import => {
                imports = decode_imports(&mut Cursor::new(&payload))?;
            },
            SectionKind::Export => {
                exports = decode_exports(&mut Cursor::new(&payload))?;
            },
            _ => {},
        }

        sections.push(WasmSection {
            kind,
            name,
            offset,
            payload_len,
        });
    }

    Ok(WasmModule {
        module_digest: Digest::sha256_of(bytes),
        size: bytes.len(),
        sections,
        imports,
        exports,
        spec_section,
        env_meta_section,
    })
}

/// Computes the SHA-256 digest of a module's bytes.
///
/// Separate from [`parse_module`] so that a caller holding bytes it cannot parse
/// can still name them. A module that fails to parse still has an identity, and
/// refusing to compute it would make an unparseable module unattributable.
#[must_use]
pub fn digest_of(bytes: &[u8]) -> Digest {
    Digest::sha256_of(bytes)
}

/// Whether a byte string begins with the WebAssembly magic number.
///
/// A cheap pre-check for a caller deciding whether a value is plausibly a module
/// before paying to parse it.
#[must_use]
pub fn looks_like_module(bytes: &[u8]) -> bool {
    bytes.len() >= WASM_MAGIC.len() && bytes[..WASM_MAGIC.len()] == WASM_MAGIC
}

/// Builds a `MalformedModule` error from a detail string.
fn malformed(detail: String) -> EngineError {
    InspectionFailure::MalformedModule { detail }.into_error()
}

/// A bounds-checked reader over a byte slice.
///
/// Every read is checked against the remaining length, so a truncated module
/// produces a named failure rather than a panic. This is the property that lets the
/// fuzz target feed arbitrary bytes to [`parse_module`] and get either a module or
/// an error, never a crash.
struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    const fn position(&self) -> usize {
        self.position
    }

    const fn has_remaining(&self) -> bool {
        self.position < self.bytes.len()
    }

    fn rest(&self) -> &'a [u8] {
        &self.bytes[self.position..]
    }

    /// Reads one byte.
    fn u8(&mut self) -> Result<u8> {
        let byte = *self.bytes.get(self.position).ok_or_else(|| {
            malformed(format!(
                "expected a byte at offset {}, but the module ends there",
                self.position
            ))
        })?;
        self.position += 1;
        Ok(byte)
    }

    /// Reads `len` bytes.
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.position.checked_add(len).ok_or_else(|| {
            malformed(format!(
                "a length of {len} at offset {} overflows",
                self.position
            ))
        })?;
        let slice = self.bytes.get(self.position..end).ok_or_else(|| {
            malformed(format!(
                "a value at offset {} declares {len} bytes, but only {} remain",
                self.position,
                self.bytes.len().saturating_sub(self.position)
            ))
        })?;
        self.position = end;
        Ok(slice)
    }

    /// Reads an unsigned LEB128 integer, bounded to 32 bits.
    ///
    /// The bound is enforced rather than relying on the shift wrapping: a
    /// continuation run longer than five bytes cannot encode a 32-bit value, and
    /// accepting one would mean reading a count that no encoder produced.
    fn u32_leb(&mut self) -> Result<u32> {
        let mut result: u32 = 0;
        let mut shift: u32 = 0;
        loop {
            let byte = self.u8()?;
            let low = u32::from(byte & 0x7f);
            if shift >= 32 || (shift == 28 && low > 0x0f) {
                return Err(malformed(format!(
                    "an unsigned LEB128 integer at offset {} does not fit in 32 bits",
                    self.position
                )));
            }
            result |= low << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
    }

    /// Reads a length-prefixed name.
    ///
    /// Decoded lossily rather than strictly: a name is a byte string and need not
    /// be UTF-8, so an invalid sequence is replaced rather than rejected. A
    /// non-UTF-8 name in a custom section means the section is not the one being
    /// looked for, which the caller decides by comparison, not a reason to fail the
    /// whole parse. `parse_module` reads *identifying names* - section names, import
    /// and export names - so replacing a stray byte with `U+FFFD` keeps the summary
    /// usable while the error would discard a perfectly measurable module.
    fn name(&mut self) -> Result<String> {
        let len = self.u32_leb()? as usize;
        let raw = self.take(len)?;
        Ok(String::from_utf8_lossy(raw).into_owned())
    }
}

/// Decodes the import section.
fn decode_imports(cursor: &mut Cursor<'_>) -> Result<Vec<WasmImport>> {
    let count = cursor.u32_leb()?;
    if count > MAX_DECODED_ENTRIES {
        return Err(malformed(format!(
            "the import section declares {count} imports, above the bound of {MAX_DECODED_ENTRIES}"
        )));
    }
    let mut imports = Vec::with_capacity(count.min(64) as usize);
    for index in 0..count {
        let module = cursor.name()?;
        let field = cursor.name()?;
        let kind_byte = cursor.u8()?;
        let kind = match kind_byte {
            0x00 => ImportKind::Function(cursor.u32_leb()?),
            0x01 => {
                skip_table_type(cursor)?;
                ImportKind::Table
            },
            0x02 => {
                skip_limits(cursor)?;
                ImportKind::Memory
            },
            0x03 => {
                // A global's type is a value type followed by a mutability byte.
                cursor.u8()?;
                cursor.u8()?;
                ImportKind::Global
            },
            0x04 => {
                // A tag's attribute byte is followed by its type index.
                cursor.u8()?;
                cursor.u32_leb()?;
                ImportKind::Tag
            },
            other => {
                return Err(malformed(format!(
                    "import {index} ({module}.{field}) has kind {other:#04x}, which is not a \
                     defined import kind"
                )));
            },
        };
        imports.push(WasmImport {
            module,
            field,
            kind,
        });
    }
    Ok(imports)
}

/// Decodes the export section's names.
fn decode_exports(cursor: &mut Cursor<'_>) -> Result<Vec<WasmExport>> {
    let count = cursor.u32_leb()?;
    if count > MAX_DECODED_ENTRIES {
        return Err(malformed(format!(
            "the export section declares {count} exports, above the bound of {MAX_DECODED_ENTRIES}"
        )));
    }
    let mut exports = Vec::with_capacity(count.min(64) as usize);
    for _ in 0..count {
        let name = cursor.name()?;
        let kind_byte = cursor.u8()?;
        let kind = match kind_byte {
            0x00 => "FUNCTION",
            0x01 => "TABLE",
            0x02 => "MEMORY",
            0x03 => "GLOBAL",
            other => {
                return Err(malformed(format!(
                    "export {name:?} has kind {other:#04x}, which is not a defined export kind"
                )));
            },
        };
        cursor.u32_leb()?;
        exports.push(WasmExport {
            name,
            kind: kind.to_owned(),
        });
    }
    Ok(exports)
}

/// Skips a table type: a reference type byte followed by its limits.
fn skip_table_type(cursor: &mut Cursor<'_>) -> Result<()> {
    let reference = cursor.u8()?;
    // The two reference types a table may hold. Externref appears in the
    // reference-types proposal, which Soroban's toolchain targets.
    if reference != 0x70 && reference != 0x6f {
        return Err(malformed(format!(
            "a table's element type is {reference:#04x}, which is neither funcref (0x70) nor \
             externref (0x6f)"
        )));
    }
    skip_limits(cursor)
}

/// Skips a resizable limits structure.
fn skip_limits(cursor: &mut Cursor<'_>) -> Result<()> {
    let flags = cursor.u8()?;
    if flags > 0x01 {
        return Err(malformed(format!(
            "a limits structure has flags {flags:#04x}, which is neither 0x00 (minimum only) nor \
             0x01 (minimum and maximum)"
        )));
    }
    cursor.u32_leb()?;
    if flags == 0x01 {
        cursor.u32_leb()?;
    }
    Ok(())
}

/// Recomputes a module's digest and compares it against a recorded value.
///
/// The free-function form of [`WasmModule::verify_deployed_digest`], for a caller
/// that has bytes but has not parsed them - which is the useful case, because a
/// module whose framing is unreadable still has to be attributable.
///
/// # Errors
///
/// Returns a provenance contradiction when the digests differ.
pub fn verify_digest(recorded: &Digest, bytes: &[u8]) -> Result<Digest> {
    let computed = digest_of(bytes);
    if computed.matches(recorded) {
        Ok(computed)
    } else {
        Err(InspectionFailure::DigestContradiction {
            contract_id: String::new(),
            recorded: recorded.value().to_owned(),
            computed: computed.value().to_owned(),
        }
        .into_error())
    }
}

/// The algorithm a module's identity uses.
///
/// Always SHA-256, and named here so that the dependency on the network's choice
/// of algorithm is visible rather than implicit in a call to `sha256_of`.
#[must_use]
pub const fn identity_algorithm() -> DigestAlgorithm {
    DigestAlgorithm::Sha256
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes a section with its identifier and LEB128 length.
    fn section(id: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![id];
        out.extend(leb(payload.len() as u32));
        out.extend_from_slice(payload);
        out
    }

    /// Encodes an unsigned LEB128 value.
    fn leb(mut value: u32) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut byte = u8::try_from(value & 0x7f).expect("7 bits fit in a byte");
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                return out;
            }
        }
    }

    /// Encodes a length-prefixed name.
    fn name(value: &str) -> Vec<u8> {
        let mut out = leb(value.len() as u32);
        out.extend_from_slice(value.as_bytes());
        out
    }

    /// Builds a module from sections, with the magic number and version.
    fn module(sections: &[Vec<u8>]) -> Vec<u8> {
        let mut out = WASM_MAGIC.to_vec();
        out.extend_from_slice(&WASM_VERSION);
        for section in sections {
            out.extend_from_slice(section);
        }
        out
    }

    /// Builds a custom section carrying a contract specification payload.
    fn spec_section(payload: &[u8]) -> Vec<u8> {
        let mut inner = name(CONTRACT_SPEC_SECTION);
        inner.extend_from_slice(payload);
        section(0, &inner)
    }

    /// Builds an import section naming one `env` function.
    fn env_import(field: &str) -> Vec<u8> {
        let mut payload = leb(1);
        payload.extend(name("env"));
        payload.extend(name(field));
        payload.push(0x00);
        payload.extend(leb(0));
        section(2, &payload)
    }

    #[test]
    fn a_minimal_module_parses_and_reports_its_digest() {
        let bytes = module(&[]);
        let parsed = parse_module(&bytes).expect("a bare module is valid");

        assert_eq!(parsed.size, 8);
        assert!(parsed.sections.is_empty());
        assert_eq!(parsed.module_digest, Digest::sha256_of(&bytes));
        assert!(!parsed.declares_interface());
        assert_eq!(identity_algorithm(), DigestAlgorithm::Sha256);
    }

    #[test]
    fn a_module_that_is_not_wasm_is_rejected_with_the_bytes_it_found() {
        let error = parse_module(b"not a module at all").expect_err("the magic number is wrong");
        // The bytes are reported without separators, which is how `hex::encode`
        // renders them; the expectation names the real rendering rather than a
        // spaced form that would never appear.
        let message = error.to_string();
        assert!(message.contains("magic number"), "got: {message}");
        assert!(
            message.contains("6e6f7420"),
            "the error must show what was found: {message}"
        );
    }

    #[test]
    fn a_truncated_header_is_rejected_rather_than_panicking() {
        parse_module(b"\0asm").expect_err("the version is missing");
        parse_module(b"\0asm\x01").expect_err("the version is truncated");
        parse_module(b"").expect_err("nothing at all is not a module");
    }

    #[test]
    fn an_unsupported_binary_version_is_named_in_the_failure() {
        let mut bytes = WASM_MAGIC.to_vec();
        bytes.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]);
        let error = parse_module(&bytes).expect_err("version 2 is not supported");
        assert!(error.to_string().contains("02000000"), "got: {error}");
    }

    #[test]
    fn every_section_kind_round_trips_through_its_identifier() {
        for id in 0..=MAX_KNOWN_SECTION_ID {
            let kind = SectionKind::from_id(id).expect("a known identifier");
            assert_eq!(kind.id(), id);
            assert!(!kind.as_str().is_empty());
        }
        assert_eq!(SectionKind::from_id(MAX_KNOWN_SECTION_ID + 1), None);
    }

    #[test]
    fn sections_are_located_with_their_offsets_and_lengths() {
        let spec = spec_section(b"\x00\x00\x00\x01");
        let bytes = module(&[spec.clone(), env_import("log")]);
        let parsed = parse_module(&bytes).expect("parses");

        assert_eq!(parsed.sections.len(), 2);
        assert_eq!(parsed.sections[0].kind, SectionKind::Custom);
        assert_eq!(
            parsed.sections[0].name.as_deref(),
            Some(CONTRACT_SPEC_SECTION)
        );
        assert_eq!(parsed.sections[0].offset, 8, "the footer starts at byte 8");
        assert_eq!(
            parsed.sections[0].payload_len,
            spec.len() - 2,
            "the recorded length excludes the identifier and the length prefix"
        );
        assert_eq!(parsed.sections[1].kind, SectionKind::Import);
        assert_eq!(parsed.sections[1].name, None);
        assert_eq!(parsed.sections_of(SectionKind::Import).len(), 1);
    }

    #[test]
    fn the_contract_specification_section_is_extracted_without_its_name() {
        // The payload is the entries, not the name: a decoder that received the
        // name too would fail on its first byte.
        let entries = b"\x00\x00\x00\x00\x00\x00\x00\x01";
        let parsed = parse_module(&module(&[spec_section(entries)])).expect("parses");

        assert!(parsed.declares_interface());
        let payload = parsed.spec_section.as_deref().expect("present");
        assert_eq!(payload, entries);
        assert_ne!(payload, CONTRACT_SPEC_SECTION.as_bytes());
    }

    #[test]
    fn the_environment_metadata_section_is_recognised_separately() {
        let mut inner = name(CONTRACT_ENV_META_SECTION);
        inner.extend_from_slice(b"\x07");
        let parsed = parse_module(&module(&[section(0, &inner)])).expect("parses");

        assert_eq!(parsed.env_meta_section.as_deref(), Some(&b"\x07"[..]));
        assert!(
            !parsed.declares_interface(),
            "the metadata section is not the interface section"
        );
    }

    #[test]
    fn an_unknown_custom_section_is_recorded_but_not_confused_for_the_interface() {
        let mut inner = name("producers");
        inner.extend_from_slice(b"\x01\x02");
        let parsed = parse_module(&module(&[section(0, &inner)])).expect("parses");

        assert_eq!(parsed.sections.len(), 1);
        assert_eq!(parsed.sections[0].name.as_deref(), Some("producers"));
        assert!(!parsed.declares_interface());
    }

    #[test]
    fn env_imports_are_separated_from_imports_of_other_namespaces() {
        // Soroban host functions live in `env`. Returning every import and trusting
        // a caller to filter would invite the filter to be forgotten.
        let mut payload = leb(2);
        payload.extend(name("env"));
        payload.extend(name("log"));
        payload.push(0x00);
        payload.extend(leb(3));
        payload.extend(name("other"));
        payload.extend(name("helper"));
        payload.push(0x00);
        payload.extend(leb(1));
        let parsed = parse_module(&module(&[section(2, &payload)])).expect("parses");

        assert_eq!(parsed.imports.len(), 2);
        assert_eq!(parsed.imports[0].module, "env");
        assert_eq!(parsed.imports[0].kind, ImportKind::Function(3));

        let host = parsed.host_function_imports();
        assert_eq!(host.len(), 1);
        assert_eq!(host[0].field, "log");
    }

    #[test]
    fn non_function_imports_are_decoded_and_skipped_correctly() {
        // Each kind has a different descriptor length, so a wrong skip would make
        // the following import unreadable rather than merely wrong.
        let mut payload = leb(4);
        payload.extend(name("m"));
        payload.extend(name("table"));
        payload.push(0x01); // table
        payload.push(0x70); // funcref
        payload.push(0x00); // minimum only
        payload.extend(leb(1));
        payload.extend(name("m"));
        payload.extend(name("memory"));
        payload.push(0x02); // memory
        payload.push(0x01); // minimum and maximum
        payload.extend(leb(1));
        payload.extend(leb(2));
        payload.extend(name("m"));
        payload.extend(name("global"));
        payload.push(0x03); // global
        payload.push(0x7f); // i32
        payload.push(0x01); // mutable
        payload.extend(name("m"));
        payload.extend(name("tag"));
        payload.push(0x04); // tag
        payload.push(0x00); // attribute
        payload.extend(leb(5));

        let parsed = parse_module(&module(&[section(2, &payload)])).expect("parses");
        assert_eq!(parsed.imports.len(), 4);
        assert_eq!(parsed.imports[0].kind, ImportKind::Table);
        assert_eq!(parsed.imports[1].kind, ImportKind::Memory);
        assert_eq!(parsed.imports[2].kind, ImportKind::Global);
        assert_eq!(parsed.imports[3].kind, ImportKind::Tag);
        assert!(parsed.host_function_imports().is_empty());
    }

    #[test]
    fn exports_are_decoded_with_their_kinds() {
        let mut payload = leb(2);
        payload.extend(name("init"));
        payload.push(0x00);
        payload.extend(leb(0));
        payload.extend(name("memory"));
        payload.push(0x02);
        payload.extend(leb(0));
        let parsed = parse_module(&module(&[section(7, &payload)])).expect("parses");

        assert_eq!(parsed.exports.len(), 2);
        assert_eq!(parsed.exports[0].name, "init");
        assert_eq!(parsed.exports[0].kind, "FUNCTION");
        assert_eq!(parsed.exports[1].kind, "MEMORY");
    }

    #[test]
    fn a_section_whose_length_overruns_the_module_is_rejected() {
        let mut bytes = module(&[]);
        bytes.push(2); // import
        bytes.push(0x7f); // a 127-byte payload that is not there
        let error = parse_module(&bytes).expect_err("the section overruns the module");
        assert!(error.to_string().contains("only 0 remain"), "got: {error}");
    }

    #[test]
    fn a_repeated_non_custom_section_is_rejected_as_corruption() {
        // A conforming encoder cannot emit this, so it indicates corruption rather
        // than an unusual module.
        let bytes = module(&[env_import("a"), env_import("b")]);
        let error = parse_module(&bytes).expect_err("two import sections");
        assert!(error.to_string().contains("more than once"), "got: {error}");
    }

    #[test]
    fn an_unknown_section_identifier_is_refused_rather_than_skipped() {
        let error =
            parse_module(&module(&[section(99, b"\x00")])).expect_err("section 99 is not defined");
        assert!(error.to_string().contains("99"), "got: {error}");
        assert!(
            error.to_string().contains("partly read"),
            "the failure must say why silence would be wrong: {error}"
        );
    }

    #[test]
    fn an_import_count_above_the_bound_is_refused_before_allocating() {
        let mut payload = leb(MAX_DECODED_ENTRIES + 1);
        payload.extend(name("env"));
        payload.extend(name("log"));
        payload.push(0x00);
        payload.extend(leb(0));
        let error = parse_module(&module(&[section(2, &payload)]))
            .expect_err("the count exceeds the bound");
        assert!(error.to_string().contains("bound"), "got: {error}");
    }

    #[test]
    fn an_import_kind_outside_the_defined_set_is_rejected() {
        let mut payload = leb(1);
        payload.extend(name("env"));
        payload.extend(name("odd"));
        payload.push(0x09);
        let error =
            parse_module(&module(&[section(2, &payload)])).expect_err("0x09 is not an import kind");
        assert!(error.to_string().contains("0x09"), "got: {error}");
    }

    #[test]
    fn a_leb128_integer_wider_than_32_bits_is_rejected() {
        let mut cursor = Cursor::new(&[0xff, 0xff, 0xff, 0xff, 0x1f]);
        cursor
            .u32_leb()
            .expect_err("six continuation bytes cannot be a u32");

        let mut cursor = Cursor::new(&[0xff, 0xff, 0xff, 0xff, 0x0f]);
        assert_eq!(cursor.u32_leb().expect("the largest u32"), u32::MAX);
    }

    #[test]
    fn a_name_that_is_not_utf8_is_summarised_rather_than_failing_the_parse() {
        // A name is a byte string, and a module with a stray byte in an unrelated
        // custom section's name is still measurable.
        let mut inner = leb(2);
        inner.extend_from_slice(&[0xff, 0xfe]);
        inner.extend_from_slice(b"\x01");
        let parsed = parse_module(&module(&[section(0, &inner)])).expect("parses");
        assert!(
            parsed.sections[0]
                .name
                .as_deref()
                .expect("a name")
                .contains('\u{fffd}'),
            "the invalid bytes are replaced, not silently dropped"
        );
    }

    #[test]
    fn a_digest_of_a_module_matches_the_network_convention() {
        // The network records SHA-256 over the module's bytes, so this must be the
        // same value a rebuild produces.
        let bytes = module(&[]);
        assert_eq!(digest_of(&bytes), Digest::sha256_of(&bytes));
        assert_eq!(digest_of(&bytes).value().len(), 64);
        assert!(looks_like_module(&bytes));
        assert!(!looks_like_module(b"nope"));
        assert!(!looks_like_module(b"\0as"));
    }

    #[test]
    fn a_matching_digest_verifies_and_a_mismatch_contradicts() {
        let bytes = module(&[]);
        let recorded = digest_of(&bytes);
        verify_digest(&recorded, &bytes).expect("the same bytes hash the same way");

        let different = digest_of(b"something else");
        let error = verify_digest(&different, &bytes)
            .expect_err("different bytes cannot hash to the same digest");
        let message = error.to_string();
        assert!(message.contains(&recorded.value().to_owned()));
        assert!(message.contains("cannot both be correct"), "got: {message}");
    }

    #[test]
    fn the_modules_own_verification_agrees_with_the_free_function() {
        let bytes = module(&[env_import("log")]);
        let parsed = parse_module(&bytes).expect("parses");
        let recorded = digest_of(&bytes);

        parsed
            .verify_deployed_digest(&recorded)
            .expect("the module's own digest is the recorded one");
        assert_eq!(parsed.module_digest, recorded);
    }

    #[test]
    fn a_module_whose_framing_is_unreadable_still_has_an_identity() {
        // A module that fails to parse is still attributable, which is what lets a
        // report describe it rather than omit it.
        let garbage = b"\0asm\x01\x00\x00\x00\x63\xff";
        parse_module(garbage).expect_err("framing is unreadable");
        assert_eq!(digest_of(garbage).algorithm(), DigestAlgorithm::Sha256);
    }

    #[test]
    fn parsing_the_same_bytes_twice_produces_identical_modules() {
        // Determinism: a report generated twice from one observation must not differ.
        let bytes = module(&[env_import("log"), spec_section(b"\x01\x02\x03")]);
        let first = parse_module(&bytes).expect("parses");
        let second = parse_module(&bytes).expect("parses");
        assert_eq!(first, second);
        assert_eq!(
            serde_json::to_string(&first).expect("serialises"),
            serde_json::to_string(&second).expect("serialises")
        );
    }
}
