//! Binding configured monitors to the displays actually present right now.
//!
//! The rule that shapes this module: a write is only issued when exactly one
//! physical display can be identified for a configured entry. Monitor numbers
//! are never used as a fallback, because they are discovery-time addresses
//! that move when a display sleeps, a dock re-enumerates, or a cable is
//! replugged. Writing the right input code to the wrong monitor is the exact
//! failure that leaves someone with two blank screens.

use crate::config::{Config, MonitorConfig};
use crate::types::{DetectedMonitor, MonitorHandle};

/// A configured monitor successfully matched to exactly one present display.
#[derive(Debug, Clone)]
pub struct Binding<'a> {
    pub monitor: &'a MonitorConfig,
    pub detected: DetectedMonitor,
    /// Non-blocking observations worth printing.
    pub notes: Vec<String>,
}

impl Binding<'_> {
    pub fn handle(&self) -> MonitorHandle {
        self.detected.handle()
    }

    pub fn logical_id(&self) -> &str {
        &self.monitor.logical_id
    }
}

/// A reason one configured monitor could not be safely addressed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindingProblem {
    #[error("`{logical_id}` is not attached: nothing present matches {looked_for}")]
    NotFound {
        logical_id: String,
        looked_for: String,
    },
    #[error("`{logical_id}` is ambiguous: {} displays match ({}). Refusing to guess.",
            candidates.len(), candidates.join(", "))]
    Ambiguous {
        logical_id: String,
        candidates: Vec<String>,
    },
    #[error("`{logical_id}` (serial {serial}) is now on connection `{found}` but was configured on `{configured}`. Cabling changed, so its verified input codes may point at the wrong physical port. Re-run `desktop-switcher configure` before switching.")]
    PortChanged {
        logical_id: String,
        serial: String,
        configured: String,
        found: String,
    },
    #[error("`{logical_id}` is reachable over {transport}, which cannot carry VCP 0x60")]
    NotSwitchable {
        logical_id: String,
        transport: String,
    },
}

impl BindingProblem {
    pub fn logical_id(&self) -> &str {
        match self {
            BindingProblem::NotFound { logical_id, .. }
            | BindingProblem::Ambiguous { logical_id, .. }
            | BindingProblem::PortChanged { logical_id, .. }
            | BindingProblem::NotSwitchable { logical_id, .. } => logical_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BindingSet<'a> {
    pub bound: Vec<Binding<'a>>,
    pub problems: Vec<BindingProblem>,
    /// Switchable displays present on this host that no config entry claims.
    pub unclaimed: Vec<DetectedMonitor>,
}

impl<'a> BindingSet<'a> {
    pub fn is_complete(&self) -> bool {
        self.problems.is_empty()
    }

    pub fn get(&self, logical_id: &str) -> Option<&Binding<'a>> {
        self.bound.iter().find(|b| b.logical_id() == logical_id)
    }
}

/// Match every configured monitor against the displays present now.
pub fn bind<'a>(config: &'a Config, detected: &[DetectedMonitor]) -> BindingSet<'a> {
    let switchable: Vec<&DetectedMonitor> = detected
        .iter()
        .filter(|d| d.identity.transport.supports_input_switching())
        .collect();

    let mut bound = Vec::new();
    let mut problems = Vec::new();

    for cfg in &config.monitors {
        match resolve_one(cfg, detected, &switchable) {
            Ok(binding) => bound.push(binding),
            Err(problem) => problems.push(problem),
        }
    }

    let unclaimed = switchable
        .iter()
        .filter(|d| {
            !bound
                .iter()
                .any(|b: &Binding| b.detected.identity.backend_id == d.identity.backend_id)
        })
        .map(|d| (*d).clone())
        .collect();

    BindingSet {
        bound,
        problems,
        unclaimed,
    }
}

fn resolve_one<'a>(
    cfg: &'a MonitorConfig,
    all: &[DetectedMonitor],
    switchable: &[&DetectedMonitor],
) -> Result<Binding<'a>, BindingProblem> {
    // The EDID serial follows the panel, so prefer it whenever we recorded one.
    if let Some(want_serial) = cfg.serial.as_deref() {
        let matches: Vec<&&DetectedMonitor> = switchable
            .iter()
            .filter(|d| d.identity.serial.as_deref() == Some(want_serial))
            .collect();

        return match matches.len() {
            1 => {
                let detected = *matches[0];
                // Same panel, different connection: the cable moved. The
                // verified codes describe which monitor-side port each computer
                // occupies, and that assumption no longer holds.
                if detected.identity.backend_id != cfg.backend_id {
                    return Err(BindingProblem::PortChanged {
                        logical_id: cfg.logical_id.clone(),
                        serial: want_serial.to_string(),
                        configured: cfg.backend_id.clone(),
                        found: detected.identity.backend_id.clone(),
                    });
                }
                Ok(Binding {
                    monitor: cfg,
                    detected: detected.clone(),
                    notes: Vec::new(),
                })
            }
            0 => {
                // Distinguish "unplugged" from "present but not switchable".
                if let Some(d) = all
                    .iter()
                    .find(|d| d.identity.serial.as_deref() == Some(want_serial))
                {
                    return Err(BindingProblem::NotSwitchable {
                        logical_id: cfg.logical_id.clone(),
                        transport: d.identity.transport.to_string(),
                    });
                }
                Err(BindingProblem::NotFound {
                    logical_id: cfg.logical_id.clone(),
                    looked_for: format!("serial {want_serial}"),
                })
            }
            _ => Err(BindingProblem::Ambiguous {
                logical_id: cfg.logical_id.clone(),
                candidates: matches
                    .iter()
                    .map(|d| d.identity.backend_id.clone())
                    .collect(),
            }),
        };
    }

    // No serial was recorded, which means the panel did not publish one.
    // Fall back to the port-scoped id and say so, but never to an index.
    let matches: Vec<&&DetectedMonitor> = switchable
        .iter()
        .filter(|d| d.identity.backend_id == cfg.backend_id)
        .collect();

    match matches.len() {
        1 => Ok(Binding {
            monitor: cfg,
            detected: (*matches[0]).clone(),
            notes: vec![format!(
                "`{}` has no EDID serial, so it is bound by connection id alone. \
                 Swapping cables between identical panels would silently rebind it.",
                cfg.logical_id
            )],
        }),
        0 => Err(BindingProblem::NotFound {
            logical_id: cfg.logical_id.clone(),
            looked_for: format!("connection {}", cfg.backend_id),
        }),
        _ => Err(BindingProblem::Ambiguous {
            logical_id: cfg.logical_id.clone(),
            candidates: matches
                .iter()
                .map(|d| d.identity.backend_id.clone())
                .collect(),
        }),
    }
}

/// Displays present more than once under the same serial.
///
/// Reported separately because it invalidates every binding that relies on
/// that serial, not just one.
pub fn duplicate_serials(detected: &[DetectedMonitor]) -> Vec<String> {
    let mut seen: Vec<(&str, usize)> = Vec::new();
    for d in detected {
        let Some(sn) = d.identity.serial.as_deref() else {
            continue;
        };
        match seen.iter_mut().find(|(s, _)| *s == sn) {
            Some((_, count)) => *count += 1,
            None => seen.push((sn, 1)),
        }
    }
    seen.into_iter()
        .filter(|(_, c)| *c > 1)
        .map(|(s, _)| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackendKind, DestinationMapping};
    use crate::types::{Evidence, MonitorIdentity, Transport};
    use std::collections::BTreeMap;

    fn detected(backend_id: &str, serial: Option<&str>, transport: Transport) -> DetectedMonitor {
        DetectedMonitor {
            identity: MonitorIdentity {
                backend_id: backend_id.into(),
                discovery_index: None,
                manufacturer: Some("AOC".into()),
                model: Some("27P2DG5".into()),
                serial: serial.map(String::from),
                transport,
            },
            raw: format!("raw for {backend_id}"),
        }
    }

    fn cfg_monitor(id: &str, backend_id: &str, serial: Option<&str>) -> MonitorConfig {
        let mut destinations = BTreeMap::new();
        destinations.insert(
            "windows".to_string(),
            DestinationMapping {
                input_code: "0x11".parse().unwrap(),
                verification: Evidence::UserConfirmed,
                verified_on: None,
                note: None,
            },
        );
        MonitorConfig {
            logical_id: id.into(),
            name: id.into(),
            backend_id: backend_id.into(),
            serial: serial.map(String::from),
            model: Some("27P2DG5".into()),
            destinations,
        }
    }

    fn config_with(monitors: Vec<MonitorConfig>) -> Config {
        let mut c = Config::new("host", BackendKind::Fake, "windows");
        c.monitors = monitors;
        c
    }

    #[test]
    fn binds_by_serial() {
        let config = config_with(vec![
            cfg_monitor("left", "port-a", Some("SN-L")),
            cfg_monitor("right", "port-b", Some("SN-R")),
        ]);
        let present = vec![
            detected("port-b", Some("SN-R"), Transport::DdcCi),
            detected("port-a", Some("SN-L"), Transport::DdcCi),
        ];
        let set = bind(&config, &present);
        assert!(set.is_complete(), "{:?}", set.problems);
        assert_eq!(
            set.get("left").unwrap().detected.identity.backend_id,
            "port-a"
        );
        assert_eq!(
            set.get("right").unwrap().detected.identity.backend_id,
            "port-b"
        );
    }

    #[test]
    fn reordered_discovery_does_not_change_bindings() {
        let config = config_with(vec![
            cfg_monitor("left", "port-a", Some("SN-L")),
            cfg_monitor("right", "port-b", Some("SN-R")),
        ]);
        let forwards = vec![
            detected("port-a", Some("SN-L"), Transport::DdcCi),
            detected("port-b", Some("SN-R"), Transport::DdcCi),
        ];
        let backwards: Vec<DetectedMonitor> = forwards.iter().rev().cloned().collect();
        let a = bind(&config, &forwards);
        let b = bind(&config, &backwards);
        assert_eq!(
            a.get("left").unwrap().detected.identity.backend_id,
            b.get("left").unwrap().detected.identity.backend_id
        );
    }

    #[test]
    fn a_missing_monitor_is_reported_not_substituted() {
        let config = config_with(vec![cfg_monitor("left", "port-a", Some("SN-L"))]);
        let present = vec![detected("port-b", Some("SN-R"), Transport::DdcCi)];
        let set = bind(&config, &present);
        assert!(set.bound.is_empty());
        assert!(matches!(set.problems[0], BindingProblem::NotFound { .. }));
    }

    #[test]
    fn duplicate_serials_make_the_binding_ambiguous_rather_than_arbitrary() {
        let config = config_with(vec![cfg_monitor("left", "port-a", Some("SAME"))]);
        let present = vec![
            detected("port-a", Some("SAME"), Transport::DdcCi),
            detected("port-b", Some("SAME"), Transport::DdcCi),
        ];
        let set = bind(&config, &present);
        assert!(set.bound.is_empty());
        assert!(matches!(set.problems[0], BindingProblem::Ambiguous { .. }));
        assert_eq!(duplicate_serials(&present), vec!["SAME"]);
    }

    #[test]
    fn moving_a_panel_to_another_port_blocks_the_switch() {
        let config = config_with(vec![cfg_monitor("left", "port-a", Some("SN-L"))]);
        let present = vec![detected("port-c", Some("SN-L"), Transport::DdcCi)];
        let set = bind(&config, &present);
        let problem = &set.problems[0];
        assert!(matches!(problem, BindingProblem::PortChanged { .. }));
        assert!(problem.to_string().contains("configure"));
    }

    #[test]
    fn internal_panel_is_never_bound_as_a_switch_target() {
        let config = config_with(vec![cfg_monitor("panel", "internal", Some("SN-P"))]);
        let present = vec![detected("internal", Some("SN-P"), Transport::Wmi)];
        let set = bind(&config, &present);
        assert!(set.bound.is_empty());
        assert!(matches!(
            set.problems[0],
            BindingProblem::NotSwitchable { .. }
        ));
    }

    #[test]
    fn serialless_panels_bind_by_port_with_a_warning() {
        let config = config_with(vec![cfg_monitor("left", "port-a", None)]);
        let present = vec![detected("port-a", None, Transport::DdcCi)];
        let set = bind(&config, &present);
        assert!(set.is_complete());
        assert!(!set.get("left").unwrap().notes.is_empty());
    }

    #[test]
    fn extra_displays_are_reported_as_unclaimed_not_ignored() {
        let config = config_with(vec![cfg_monitor("left", "port-a", Some("SN-L"))]);
        let present = vec![
            detected("port-a", Some("SN-L"), Transport::DdcCi),
            detected("port-z", Some("SN-Z"), Transport::DdcCi),
        ];
        let set = bind(&config, &present);
        assert_eq!(set.unclaimed.len(), 1);
        assert_eq!(set.unclaimed[0].identity.backend_id, "port-z");
    }
}
