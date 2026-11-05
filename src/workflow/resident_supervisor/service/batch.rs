//! Bounded coherent sets; two samples still bracket group/pin checks.
use super::*;
const EXTRA: &[&str] = &[
    "NotifyAccess",
    "BusName",
    "Nice",
    "Requires",
    "Wants",
    "BindsTo",
    "PartOf",
    "Before",
    "UMask",
    "KillSignal",
    "FinalKillSignal",
    "WorkingDirectory",
    "User",
    "Group",
];
pub struct ServiceBatch {
    snapshots: BTreeMap<String, Snapshot>,
    fields: BTreeMap<String, BTreeMap<String, String>>,
}
impl ServiceBatch {
    pub fn get(&self, unit: &str) -> Result<&Snapshot> {
        self.snapshots.get(unit).context("missing batch unit")
    }
    pub(super) fn policy_bytes(
        &self,
        unit: &str,
        keys: impl Iterator<Item = &'static str>,
    ) -> Result<Vec<u8>> {
        let fields = self.fields.get(unit).context("missing batch policy unit")?;
        keys.map(|key| {
            Ok(format!(
                "{key}={}\n",
                fields.get(key).context("missing batch property")?
            ))
        })
        .collect::<Result<String>>()
        .map(String::into_bytes)
    }
    fn parse(units: &[&str], bytes: &[u8]) -> Result<Self> {
        validate_units(units)?;
        ensure!(
            bytes.len() <= LIMIT,
            "service batch exceeds unchanged bound"
        );
        let text = std::str::from_utf8(bytes)?;
        ensure!(
            !text.bytes().any(|c| c.is_ascii_control() && c != b'\n'),
            "control byte in service batch"
        );
        let expected: std::collections::BTreeSet<_> =
            STATE.iter().chain(POLICY).chain(EXTRA).copied().collect();
        let mut snapshots = BTreeMap::new();
        let mut all = BTreeMap::new();
        for block in text.trim_end_matches('\n').split("\n\n") {
            let mut fields = BTreeMap::new();
            for line in block.lines() {
                let (key, value) = line.split_once('=').context("malformed batch field")?;
                ensure!(
                    expected.contains(key)
                        && fields.insert(key.to_owned(), value.to_owned()).is_none(),
                    "unknown or duplicate batch field"
                );
            }
            ensure!(fields.len() == expected.len(), "incomplete batch fields");
            let unit = fields.get("Id").context("missing batch Id")?.clone();
            ensure!(
                units.contains(&unit.as_str()) && !snapshots.contains_key(&unit),
                "unknown or duplicate batch unit"
            );
            let raw = STATE
                .iter()
                .chain(POLICY)
                .map(|key| format!("{key}={}\n", fields[*key]))
                .collect::<String>();
            snapshots.insert(unit.clone(), Snapshot::parse(raw.as_bytes(), &unit)?);
            all.insert(unit, fields);
        }
        ensure!(snapshots.len() == units.len(), "missing batch unit");
        Ok(Self {
            snapshots,
            fields: all,
        })
    }
}
fn validate_units(units: &[&str]) -> Result<()> {
    ensure!(
        !units.is_empty() && units.len() <= 5,
        "service batch unit bound"
    );
    let mut seen = std::collections::BTreeSet::new();
    for unit in units {
        validate_unit(unit)?;
        ensure!(seen.insert(*unit), "duplicate requested unit");
    }
    Ok(())
}
pub fn observe_many(
    units: &[&str],
    deadline: Instant,
    pulse: &mut dyn FnMut() -> Result<()>,
) -> Result<ServiceBatch> {
    observe_many_with(units, deadline, pulse, |command, deadline, pulse| {
        run_with_progress(command, deadline, pulse)
    })
}
fn observe_many_with(
    units: &[&str],
    deadline: Instant,
    pulse: &mut dyn FnMut() -> Result<()>,
    mut run: impl FnMut(Command, Instant, &mut dyn FnMut() -> Result<()>) -> Result<Vec<u8>>,
) -> Result<ServiceBatch> {
    validate_units(units)?;
    let mut command = Command::new("/bin/systemctl");
    command.args([
        "--system",
        "--no-pager",
        "--no-ask-password",
        "--all",
        "show",
    ]);
    for key in STATE
        .iter()
        .chain(POLICY)
        .chain(EXTRA)
        .collect::<std::collections::BTreeSet<_>>()
    {
        command.arg(format!("--property={key}"));
    }
    command.arg("--").args(units);
    ServiceBatch::parse(units, &run(command, deadline, pulse)?)
}

#[cfg(test)]
pub(crate) mod tests;
