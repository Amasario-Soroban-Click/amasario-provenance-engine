//! `amasario discover` - which other contracts were observed in the target's company.
//!
//! Discovery answers a narrower question than `dependencies`. It reports every contract
//! the bounded observation mentioned - the callees the target is known to have entered,
//! the callers that entered the target, and the imports the module declares - without
//! deciding whether any of them is a dependency. That decision belongs to the dependency
//! layer, which requires a basis and evidence per edge, and it is not made here.
//!
//! # Why the two are separate commands
//!
//! A contract that was merely mentioned is not a dependency. If discovery reported
//! mentions as dependencies, the specification's central prohibition - that a dependency
//! must not be inferred because two projects mention one another - would be violated by
//! the tool that exists to enforce it. Discovery therefore names what was observed and
//! says nothing about what it means.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use amasario_core::Result;

use crate::config::{BoundsArgs, OutputArgs, OutputFormat, TargetArgs};
use crate::errors::{CliError, CliResult};
use crate::output::emit;

use super::{observe, to_value};

/// One contract observed in the target's company, and how.
struct Mention {
    contract: String,
    /// The functions the target was observed to enter, when it entered this contract.
    entered: Vec<String>,
    /// Whether this contract was observed entering the target.
    entered_target: bool,
    /// Transactions in which the mention was seen, deduplicated and in order.
    transactions: Vec<String>,
}

impl Mention {
    const fn new(contract: String) -> Self {
        Self {
            contract,
            entered: Vec::new(),
            entered_target: false,
            transactions: Vec::new(),
        }
    }
}

/// Runs the command.
///
/// # Errors
///
/// Returns the observation's failure, a usage error for a format the command cannot
/// render, and a validation error when the target address is malformed.
pub async fn run(target: TargetArgs, bounds: BoundsArgs, output: OutputArgs) -> CliResult<()> {
    // Discovery is about the target's company, which is only visible through the events
    // the contract emitted and the transactions that entered it. A run that skipped the
    // scan would report nothing and mean "nothing was found" when it meant "nothing was
    // looked for", so the scan is forced on here rather than left to a flag.
    let bounds = BoundsArgs {
        scan_events: true,
        ..bounds
    };
    let inspection = observe(&target, &bounds).await?;

    let mut mentions: BTreeMap<String, Mention> = BTreeMap::new();
    for invocation in &inspection.invocations {
        let transaction = invocation.transaction.to_string();

        if invocation.callee != target.contract {
            let entry = mentions
                .entry(invocation.callee.clone())
                .or_insert_with(|| Mention::new(invocation.callee.clone()));
            if let Some(function) = &invocation.function
                && !entry.entered.contains(function)
            {
                entry.entered.push(function.clone());
            }
            if !entry.transactions.contains(&transaction) {
                entry.transactions.push(transaction.clone());
            }
        }

        if let Some(caller) = &invocation.caller
            && caller != &target.contract
        {
            let entry = mentions
                .entry(caller.clone())
                .or_insert_with(|| Mention::new(caller.clone()));
            entry.entered_target = true;
            if !entry.transactions.contains(&transaction) {
                entry.transactions.push(transaction.clone());
            }
        }
    }

    let mentions: Vec<Mention> = mentions.into_values().collect();

    let rendered = match output.format {
        OutputFormat::Json => {
            let value = view(&inspection, &mentions)?;
            if output.pretty {
                serde_json::to_string_pretty(&value)?
            } else {
                serde_json::to_string(&value)?
            }
        },
        OutputFormat::Text => text(&target.contract, &inspection, &mentions),
        OutputFormat::Markdown | OutputFormat::Dot | OutputFormat::Junit => {
            return Err(CliError::Usage(format!(
                "discover renders text or json, not {}",
                output.format.as_str()
            )));
        },
    };

    emit(&rendered, output.output.as_deref())?;
    Ok(())
}

/// The published JSON view.
fn view(inspection: &amasario_contract::ContractInspection, mentions: &[Mention]) -> Result<Value> {
    let entries: Vec<Value> = mentions
        .iter()
        .map(|mention| {
            json!({
                "contract": mention.contract,
                "entered": mention.entered,
                "enteredTarget": mention.entered_target,
                "transactions": mention.transactions,
            })
        })
        .collect();

    Ok(json!({
        "subject": inspection.identity.contract_id.to_string(),
        "network": inspection.identity.network_id,
        "mentions": entries,
        "boundary": to_value(&inspection.boundary)?,
        "transactionsRead": inspection.transactions_read,
        "truncated": inspection.is_truncated(),
        "note": "A mention is not a dependency. Use `amasario dependencies` for edges \
                 that have a defined basis and evidence.",
    }))
}

/// The human-readable view.
fn text(
    subject: &str,
    inspection: &amasario_contract::ContractInspection,
    mentions: &[Mention],
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "discovery   {subject}");
    let _ = writeln!(
        out,
        "observed    {} transaction(s) read, {} invocation(s) seen",
        inspection.transactions_read,
        inspection.invocations.len()
    );
    let _ = writeln!(out);

    if mentions.is_empty() {
        if inspection.is_truncated() {
            let _ = writeln!(
                out,
                "no contract was observed, and the scan stopped early, so this is not a \
                 statement that none exists"
            );
        } else {
            let _ = writeln!(
                out,
                "no other contract was observed. The bounded scan found no cross-contract \
                 call, which is not the same as the contract having no callers or callees."
            );
        }
    } else {
        for mention in mentions {
            let direction = if mention.entered.is_empty() && mention.entered_target {
                "entered the target"
            } else if mention.entered_target {
                "both entered the target and was entered"
            } else {
                "was entered by the target"
            };
            let _ = writeln!(out, "{} - {direction}", mention.contract);
            if !mention.entered.is_empty() {
                let _ = writeln!(out, "  functions: {}", mention.entered.join(", "));
            }
            let _ = writeln!(out, "  transactions: {}", mention.transactions.join(", "));
        }
    }

    let _ = writeln!(out);
    let _ = write!(
        out,
        "A mention is not a dependency. `amasario dependencies` establishes which of \
         these hold on evidence."
    );
    out
}
