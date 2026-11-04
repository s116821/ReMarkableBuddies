use super::*;
use crate::workflow::resident_runtime::{ReadOnlyAdmission, ReadOnlyRequest};
use crate::workflow::resident_supervisor::service::{companion as broker, ServiceBatch};
struct Admission {
    polls: usize,
    commands: usize,
    pulses: usize,
    boundaries: usize,
}
impl ReadOnlyAdmission for Admission {
    fn pulse(&mut self, deadline: Instant) -> Result<()> {
        request::before(deadline, "test original deadline")?;
        self.pulses += 1;
        Ok(())
    }
    fn boundary(&mut self, deadline: Instant) -> Result<()> {
        self.boundaries += 1;
        for _ in 0..2 {
            let mut local = 0;
            let batch = ServiceBatch::fixture_observe(
                &["xochitl.service", "rm-sync.service"],
                deadline,
                self.polls,
                &mut local,
                &mut || self.pulse(deadline),
            )?;
            self.commands += local;
            ensure!(
                batch.get("rm-sync.service")?.property("Id") == Some("rm-sync.service"),
                "wrong sampled unit"
            );
        }
        Ok(())
    }
}
#[test]
#[ignore = "requires root for the fixed vendor process contract"]
fn actual_owned_pair_broker_and_pid1_command_counts_are_independent_of_poll_count() {
    let mut results = Vec::new();
    for polls in [1, 1000] {
        let fixture = Fixture::new();
        let mut runtime = fixture
            .start_count_companion_with_store(fixture.store(), Duration::from_secs(10))
            .unwrap();
        let companion = runtime.fixture_companion_pid().unwrap();
        runtime.fixture_broker_commands();
        broker::fixture_probe(polls);
        let mut admission = Admission {
            polls,
            commands: 0,
            pulses: 0,
            boundaries: 0,
        };
        let deadline = Instant::now() + Duration::from_secs(90);
        let result = runtime.request_read_only(
            OperationId(nonce()),
            ReadOnlyRequest::Observe,
            deadline,
            &mut admission,
        );
        let stderr = fs::read(runtime.evidence_directory().join("stderr.log"))
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
        assert!(
            matches!(result, Ok(Reply::Observed(_))),
            "polls={polls}; result={result:?}; retained reader stderr={stderr:?}"
        );
        assert_eq!(runtime.fixture_companion_pid(), Some(companion));
        assert_eq!(broker::fixture_queries(), 12);
        assert_eq!(admission.commands, 6);
        assert_eq!(admission.boundaries, 3);
        results.push((
            admission.commands,
            broker::fixture_queries(),
            admission.pulses,
        ));
        let report = runtime.shutdown(Instant::now() + Duration::from_secs(3));
        assert!(
            report.reaped
                && report.reader_reaped
                && report.companion.as_ref().is_some_and(|c| c.reaped),
            "{report:?}"
        );
    }
    assert_eq!(results[0].0, results[1].0);
    assert_eq!(results[0].1, results[1].1);
    assert!(results[1].2 > results[0].2 + 1000);
}
