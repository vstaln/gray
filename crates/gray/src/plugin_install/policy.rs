use gray_plugin::capabilities;
use gray_plugin::scan::{Report, Verdict};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentRecord {
    pub granted: Vec<String>,
    pub hash: String,
}

pub fn grant(declared: &[String]) -> ConsentRecord {
    let known = capabilities::parse_declared(declared);
    ConsentRecord {
        hash: capabilities::consent_hash(&known),
        granted: known,
    }
}

#[derive(Clone, Debug)]
pub struct CapabilityLine {
    pub id: String,
    pub description: String,
    pub grantable: bool,
}

pub fn capability_lines(declared: &[String]) -> Vec<CapabilityLine> {
    declared
        .iter()
        .map(|id| {
            let spec = capabilities::spec(id.trim());
            CapabilityLine {
                id: id.clone(),
                description: spec
                    .map(|s| s.description)
                    .unwrap_or("(unknown to this build)")
                    .to_string(),
                grantable: spec.is_some(),
            }
        })
        .collect()
}

pub fn require_installable_scan(report: &Report) -> anyhow::Result<()> {
    anyhow::ensure!(
        report.verdict() != Verdict::Dangerous,
        "plugin scan blocked installation: {}",
        report.summary()
    );
    Ok(())
}
