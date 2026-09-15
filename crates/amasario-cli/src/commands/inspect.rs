//! `amasario inspect` - what is deployed at an address, and what it was observed to do.
//!
//! The command reports the contract's identity, the module it executes, whether the
//! module's bytes hash to the digest the network records, the interface it declares, and
//! the invocations and events the bounded scan reached. It does not decide what the
//! contract depends on or where it came from; those are `dependencies` and `provenance`.
//!
//! # A contradiction fails the command
//!
//! When the retrieved module does not hash to the recorded digest, that is two facts that
//! cannot both be true, and the command exits with the gate code rather than the success
//! code. The report is still written - the contradiction is exactly what a caller needs
//! to see - but a CI job that ran `inspect` sees a non-zero status, because a contract
//! whose code does not match its recorded digest is not a contract whose inspection
//! succeeded.

use amasario_contract::ContractInspection;
use serde_json::{Value, json};

use crate::config::{BoundsArgs, OutputArgs, OutputFormat, TargetArgs};
use crate::errors::{CliError, CliResult};
use crate::output::emit;

use super::{observe, to_value, truncation_names};

/// Runs the command.
///
/// # Errors
///
/// Returns the network, contract or validation failure the observation produced, a usage
/// error for a format the command cannot render, and a gate failure when the inspection
/// found a contradiction.
pub async fn run(target: TargetArgs, bounds: BoundsArgs, output: OutputArgs) -> CliResult<()> {
    let inspection = observe(&target, &bounds).await?;

    let rendered = match output.format {
        OutputFormat::Json => {
            let value = view(&inspection)?;
            if output.pretty {
                serde_json::to_string_pretty(&value)?
            } else {
                serde_json::to_string(&value)?
            }
        },
        OutputFormat::Text => text(&inspection),
        OutputFormat::Markdown | OutputFormat::Dot | OutputFormat::Junit => {
            return Err(CliError::Usage(format!(
                "inspect renders text or json, not {}; use `dependencies` for a \
                 dependency listing, `graph` for DOT, and `report` for Markdown and JUnit",
                output.format.as_str()
            )));
        },
    };

    emit(&rendered, output.output.as_deref())?;

    if inspection.has_contradiction() {
        let detail = inspection
            .anomalies
            .iter()
            .find(|failure| failure.is_contradiction())
            .map(|failure| amasario_contract::describe(failure).to_owned())
            .unwrap_or_else(|| "an anomaly was recorded".to_owned());
        return Err(CliError::Gate(format!(
            "{} contradicts the state the network records: {detail}",
            target.contract
        )));
    }

    Ok(())
}

/// The published JSON view of an inspection.
///
/// A projection rather than the inspection itself, because [`ContractInspection`] is not
/// itself a serialisable type: it holds an [`InspectionFailure`](amasario_contract::InspectionFailure)
/// list, which is a control-flow enumeration rather than a document. Each part that is a
/// document is serialised as itself, so the JSON carries the identity's own field names
/// and an observer cannot mistake this for a published specification document.
fn view(inspection: &ContractInspection) -> CliResult<Value> {
    let module = match &inspection.module {
        Some(module) => json!({
            "moduleDigest": module.module_digest.value(),
            "size": module.size,
            "imports": module.imports.len(),
            "exports": module.exports.len(),
            "sections": module.sections.len(),
            "declaresInterface": module.declares_interface(),
        }),
        None => Value::Null,
    };

    let interface = match &inspection.interface {
        Some(interface) => json!({
            "functions": interface.functions.len(),
            "structs": interface.structs.len(),
            "unions": interface.unions.len(),
            "enums": interface.enums.len(),
            "errorEnums": interface.error_enums.len(),
            "events": interface.events.len(),
            "undecodableTrailingBytes": interface.undecodable_trailing_bytes,
        }),
        None => Value::Null,
    };

    Ok(json!({
        "identity": to_value(&inspection.identity)?,
        "digestVerified": inspection.digest_verified,
        "module": module,
        "interface": interface,
        "functionNames": inspection
            .interface
            .as_ref()
            .map(|interface| {
                interface
                    .functions
                    .iter()
                    .map(|function| function.name.clone())
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default(),
        "instanceStorageEntries": inspection.instance_storage.len(),
        "invocations": to_value(&inspection.invocations)?,
        "emittedEvents": to_value(&inspection.emitted_events)?,
        "anomalies": inspection
            .anomalies
            .iter()
            .map(|failure| amasario_contract::describe(failure).to_owned())
            .collect::<Vec<String>>(),
        "truncation": truncation_names(&inspection.truncation),
        "transactionsRead": inspection.transactions_read,
        "boundary": to_value(&inspection.boundary)?,
    }))
}

/// The human-readable view.
fn text(inspection: &ContractInspection) -> String {
    use std::fmt::Write as _;

    let identity = &inspection.identity;
    let mut out = String::new();
    let _ = writeln!(out, "contract    {}", identity.contract_id);
    let _ = writeln!(out, "network     {}", identity.network_id);
    let _ = writeln!(out, "executable  {}", identity.executable_kind.as_str());
    let _ = writeln!(
        out,
        "wasm hash   {}",
        identity.wasm_hash.as_ref().map_or_else(
            || "(none: not a WASM contract)".to_owned(),
            |d| d.value().to_owned()
        )
    );
    let _ = writeln!(out, "at ledger   {}", identity.resolved_at_ledger);

    match &inspection.module {
        Some(module) => {
            let _ = writeln!(
                out,
                "module      {} bytes, {} import(s), {} export(s)",
                module.size,
                module.imports.len(),
                module.exports.len()
            );
        },
        None => {
            let _ = writeln!(out, "module      (none)");
        },
    }

    let _ = writeln!(
        out,
        "digest      {}",
        match inspection.digest_verified {
            Some(true) => "verified against the recorded digest",
            Some(false) => "DOES NOT MATCH the recorded digest",
            None => "not applicable: there is no module to verify",
        }
    );

    match &inspection.interface {
        Some(interface) => {
            let _ = writeln!(
                out,
                "interface   {} function(s), {} event(s), {} type(s)",
                interface.functions.len(),
                interface.events.len(),
                interface.structs.len() + interface.unions.len() + interface.enums.len()
            );
            for function in &interface.functions {
                let _ = writeln!(out, "  fn {}({})", function.name, function.inputs.len());
            }
        },
        None => {
            let _ = writeln!(
                out,
                "interface   (none declared: the module has no specification section)"
            );
        },
    }

    let _ = writeln!(
        out,
        "storage     {} instance entr(ies)",
        inspection.instance_storage.len()
    );
    let _ = writeln!(
        out,
        "invocations {} observed, from {} transaction(s) read",
        inspection.invocations.len(),
        inspection.transactions_read
    );
    let _ = writeln!(
        out,
        "events      {} observed",
        inspection.emitted_events.len()
    );

    if inspection.anomalies.is_empty() {
        let _ = writeln!(out, "anomalies   none");
    } else {
        let _ = writeln!(out, "anomalies   {}", inspection.anomalies.len());
        for anomaly in &inspection.anomalies {
            let _ = writeln!(out, "  {}", amasario_contract::describe(anomaly));
        }
    }

    if inspection.is_truncated() {
        let _ = writeln!(
            out,
            "bounded     the search stopped early: {}",
            inspection
                .truncation
                .iter()
                .map(|reason| reason.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let _ = writeln!(
            out,
            "            this result is not a claim of completeness"
        );
    }

    let _ = writeln!(out);
    let _ = write!(
        out,
        "Amasario is not a security scanner. Nothing above is an assessment \
        of this contract's safety."
    );
    out
}
