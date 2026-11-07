//! Manual, opt-in Drive fixed-slot transport qualification.
use anyhow::{ensure, Result};
use clap::{Parser, Subcommand};
use remarkable_reader_buddy::storage::drive::{
    qualification as wire,
    slot::{
        AgreedSlotObservation, PointerMatch, PreparedSlot, SlotInvalid, SlotObservation,
        SlotPointer,
    },
    DriveTransport, GoogleDrive,
};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};

#[path = "drive_slot_qualification/ledger.rs"]
mod ledger;
use ledger::{Action, Directory, Evidence, Frozen, Journal, Plan, IDS, ROLES};

#[derive(Parser)]
#[command(about = "Explicit disposable Drive qualification; no default live action")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Reserve 27 generated IDs and freeze the approved private plan.
    Allocate {
        directory: PathBuf,
        #[arg(long)]
        plan_sha256: String,
    },
    /// Consume the frozen run once. Unknown sends are never replayed.
    Run {
        directory: PathBuf,
        #[arg(long)]
        frozen_sha256: String,
    },
    /// Print local sanitized evidence without opening any credential.
    Report { directory: PathBuf },
    /// Read only the exact IDs of recorded sends.
    Observe {
        directory: PathBuf,
        #[arg(long)]
        frozen_sha256: String,
        #[arg(long)]
        until_unix: u64,
    },
    /// Separately authorized exact-ID cleanup.
    Cleanup {
        directory: PathBuf,
        #[arg(long)]
        frozen_sha256: String,
        #[arg(long)]
        journal_sha256: String,
        #[arg(long)]
        until_unix: u64,
    },
}

fn client(dir: &Directory, plan: &Plan, role: usize, deadline: Instant) -> Result<GoogleDrive> {
    let mut client = GoogleDrive::open(dir.grant(role)?)?;
    wire::verify_binding(&client, &plan.grants[role].binding_sha256, plan.group)?;
    wire::set_window(&mut client, deadline, Arc::new(AtomicBool::new(false)));
    Ok(client)
}

fn allocate(root: &Path, approved: &str) -> Result<()> {
    let dir = Directory::open(root)?;
    let (plan, sha): (Plan, _) = dir.load("plan.json")?;
    plan.validate()?;
    plan.verify_build()?;
    ensure!(
        sha == approved
            && !dir.exists("allocation.json")
            && !dir.exists("frozen.json")
            && !dir.exists("journal.json"),
        "allocation unavailable"
    );
    let deadline = ledger::deadline(plan.expires_unix)?;
    let mut clients = (0..ROLES.len())
        .map(|role| client(&dir, &plan, role, deadline))
        .collect::<Result<Vec<_>>>()?;
    check_role_bindings(&plan, &clients)?;
    let allocator = &mut clients[0];
    let mut ids = Vec::<String>::new();
    dir.save("allocation.json", &ids)?;
    let allocator_refreshed =
        wire::authorize_refresh(allocator, plan.group, plan.grants[0].stimulate_refresh)?;
    while ids.len() < IDS {
        let id = allocator.allocate()?;
        ensure!(!ids.contains(&id), "provider returned a duplicate ID");
        ids.push(id);
        dir.save("allocation.json", &ids)?;
    }
    let frozen = Frozen {
        plan,
        ids,
        allocator_refreshed,
    };
    frozen.validate()?;
    dir.save("journal.json", &Journal::default())?;
    dir.save("frozen.json", &frozen)?;
    println!("frozen_sha256 {}", dir.frozen()?.1);
    Ok(())
}

struct Run<'a> {
    dir: &'a Directory,
    frozen: &'a Frozen,
    journal: Mutex<Journal>,
}

impl Run<'_> {
    fn update<T>(&self, operation: impl FnOnce(&mut Journal) -> Result<T>) -> Result<T> {
        let mut journal = self
            .journal
            .lock()
            .map_err(|_| anyhow::anyhow!("journal unavailable"))?;
        let result = operation(&mut journal)?;
        self.dir.save("journal.json", &*journal)?;
        Ok(result)
    }

    fn evidence(&self, line: Evidence) -> Result<()> {
        self.update(|journal| {
            journal.evidence.push(line);
            Ok(())
        })
    }

    fn reserve(&self, index: usize, action: Action, bytes: usize) -> Result<usize> {
        self.update(|journal| journal.add(index, action, bytes))
    }

    fn response(&self, reservation: usize, status: Option<u16>) -> Result<()> {
        self.update(|journal| {
            journal.sends[reservation].response = status;
            journal.sends[reservation].returned = true;
            Ok(())
        })
    }

    fn stage(&self, client: &mut GoogleDrive, index: usize) -> Result<()> {
        let resource = self.frozen.resource(index, false)?;
        let reservation = Cell::new(None);
        let result = wire::stage(client, &resource, |bytes| {
            reservation.set(Some(self.reserve(index, Action::Original, bytes)?));
            Ok(())
        });
        if let Some(row) = reservation.get() {
            self.response(row, result.as_ref().ok().copied().flatten())?;
        }
        ensure!(
            result?.is_some_and(success)
                && wire::presence(client, &resource)? == wire::Presence::Exact,
            "fixture identity unresolved"
        );
        Ok(())
    }

    fn prepare(&self, client: &mut GoogleDrive, pointer: &SlotPointer) -> Result<PreparedSlot> {
        let reference = self
            .frozen
            .descriptor(&pointer.descriptor_id)?
            .descriptor_reference()?;
        ensure!(
            reference.sha256 == pointer.descriptor_sha256,
            "descriptor digest differs"
        );
        client.prepare_slot(
            pointer.binding.clone(),
            pointer.descriptor_id.clone(),
            reference,
        )
    }

    fn create(
        &self,
        client: &mut GoogleDrive,
        index: usize,
        action: Action,
        prepared: &PreparedSlot,
        gate: Option<&Gate>,
        interrupt: bool,
    ) -> (Option<u16>, Result<SlotObservation>) {
        let reservation = Cell::new(None);
        let status = Cell::new(None);
        let result = wire::create(
            client,
            prepared,
            |bytes| {
                let row = self.reserve(index, action, bytes)?;
                reservation.set(Some(row));
                if let Some(gate) = gate {
                    if let Err(error) = gate.wait() {
                        self.update(|journal| {
                            journal.sends[row].canceled_before_send = true;
                            Ok(())
                        })?;
                        return Err(error);
                    }
                }
                Ok(())
            },
            |code| {
                let row = reservation
                    .get()
                    .ok_or_else(|| anyhow::anyhow!("missing reservation"))?;
                self.response(row, code)?;
                status.set(code);
                ensure!(!interrupt, "qualification acknowledgement interrupted");
                Ok(())
            },
        );
        (status.get(), result)
    }

    fn contend(
        &self,
        client: &mut GoogleDrive,
        index: usize,
        contender: bool,
        gate: &Gate,
    ) -> Result<(u16, SlotObservation)> {
        let result = (|| {
            let pointer = self.frozen.resource(index, contender)?.pointer()?;
            let prepared = self.prepare(client, &pointer)?;
            let action = if contender {
                Action::Contender
            } else {
                Action::Original
            };
            let (status, value) = self.create(client, index, action, &prepared, Some(gate), false);
            Ok((
                status.ok_or_else(|| anyhow::anyhow!("uncertain contender POST"))?,
                value?,
            ))
        })();
        if result.is_err() {
            gate.abort();
        }
        result
    }
}

struct Gate {
    state: Mutex<(u8, bool)>,
    changed: Condvar,
    end: Instant,
}

impl Gate {
    fn new(end: Instant) -> Self {
        Self {
            state: Mutex::new((0, false)),
            changed: Condvar::new(),
            end,
        }
    }

    fn wait(&self) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("rendezvous unavailable"))?;
        ensure!(!state.1 && Instant::now() < self.end, "rendezvous expired");
        state.0 += 1;
        if state.0 == 2 {
            self.changed.notify_all();
            return Ok(());
        }
        let (mut state, _) = self
            .changed
            .wait_timeout_while(
                state,
                self.end.saturating_duration_since(Instant::now()),
                |s| s.0 < 2 && !s.1,
            )
            .map_err(|_| anyhow::anyhow!("rendezvous unavailable"))?;
        if state.0 < 2 {
            state.1 = true;
            self.changed.notify_all();
        }
        ensure!(!state.1, "rendezvous aborted");
        Ok(())
    }

    fn abort(&self) {
        if let Ok(mut state) = self.state.lock() {
            if state.0 < 2 {
                state.1 = true;
                self.changed.notify_all();
            }
        }
    }
}

fn success(status: u16) -> bool {
    (200..300).contains(&status)
}

fn agreed(frozen: &Frozen, client: &mut GoogleDrive, pointer: &SlotPointer) -> Result<()> {
    let AgreedSlotObservation::Present(observed) = client.observe_agreed_slot(&pointer.binding)?
    else {
        anyhow::bail!("slot not visible with complete private properties");
    };
    ensure!(
        observed.pointer == *pointer
            && ledger::digest(&observed.descriptor) == pointer.descriptor_sha256
            && frozen
                .descriptor(&pointer.descriptor_id)?
                .matches_descriptor_bytes(&observed.descriptor),
        "observed slot differs"
    );
    Ok(())
}

fn create_unique(run: &Run<'_>, client: &mut GoogleDrive, index: usize) -> Result<SlotPointer> {
    let pointer = run.frozen.resource(index, false)?.pointer()?;
    let prepared = run.prepare(client, &pointer)?;
    let (status, value) = run.create(client, index, Action::Original, &prepared, None, false);
    ensure!(
        status.is_some_and(success) && value?.candidate_match == PointerMatch::Same,
        "fresh slot create did not resolve to original"
    );
    Ok(pointer)
}

fn check_role_bindings(plan: &Plan, clients: &[GoogleDrive]) -> Result<Vec<Evidence>> {
    let mut evidence = Vec::new();
    for left in 0..4 {
        for right in left + 1..4 {
            ensure!(
                wire::same_account(&clients[left], &clients[right])
                    && wire::independent_grants(&clients[left], &clients[right]),
                "independent same-account grants required"
            );
            let shared = wire::same_registration(&clients[left], &clients[right]);
            ensure!(
                shared == (plan.grants[left].binding_sha256 == plan.grants[right].binding_sha256),
                "registration mapping differs"
            );
            evidence.push(Evidence::Registration {
                left,
                right,
                shared,
            });
        }
    }
    ensure!(
        wire::same_registration(&clients[0], &clients[4])
            && !wire::same_account(&clients[0], &clients[4])
            && !wire::same_registration(&clients[0], &clients[5])
            && wire::same_account(&clients[0], &clients[5]),
        "changed account/application controls are not distinct"
    );
    Ok(evidence)
}

fn qualify_clients(run: &Run<'_>, clients: &mut [GoogleDrive]) -> Result<()> {
    let frozen = run.frozen;
    for event in check_role_bindings(&frozen.plan, clients)? {
        run.evidence(event)?;
    }
    for (role, client) in clients[..4].iter_mut().enumerate() {
        let stimulated = frozen.plan.grants[role].stimulate_refresh;
        let refreshed = if role == 0 && frozen.allocator_refreshed {
            client.authorize(frozen.plan.group)?;
            true
        } else {
            wire::authorize_refresh(client, frozen.plan.group, stimulated)?
        };
        run.evidence(Evidence::Refresh {
            role,
            stimulated,
            observed: refreshed,
        })?;
        ensure!(refreshed, "required refresh observation missing");
    }
    Ok(())
}

fn race(run: &Run<'_>, clients: &mut [GoogleDrive], index: usize, deadline: Instant) -> Result<()> {
    let end =
        deadline.min(Instant::now() + Duration::from_millis(run.frozen.plan.rendezvous_millis));
    let gate = Gate::new(end);
    let (first, second) = clients.split_at_mut(3);
    let (a, b) = (&mut first[0], &mut second[0]);
    let (left, right) = thread::scope(|scope| {
        let left = scope.spawn(|| run.contend(a, index, false, &gate));
        let right = scope.spawn(|| run.contend(b, index, true, &gate));
        (left.join(), right.join())
    });
    let (left_status, left) = left.map_err(|_| anyhow::anyhow!("contender failed"))??;
    let (right_status, right) = right.map_err(|_| anyhow::anyhow!("contender failed"))??;
    let second_won = match (left_status, right_status) {
        (s, 409) if success(s) => false,
        (409, s) if success(s) => true,
        _ => anyhow::bail!("contention response pair inconclusive"),
    };
    let winner = run.frozen.resource(index, second_won)?.pointer()?;
    let expected = if second_won {
        (PointerMatch::Different, PointerMatch::Same)
    } else {
        (PointerMatch::Same, PointerMatch::Different)
    };
    ensure!(
        left.pointer == winner
            && right.pointer == winner
            && left.descriptor == right.descriptor
            && (left.candidate_match, right.candidate_match) == expected,
        "contenders did not retain the same winner"
    );
    let mut observer = client(run.dir, &run.frozen.plan, 0, deadline)?;
    agreed(run.frozen, &mut observer, &winner)?;
    *a = observer;
    run.evidence(Evidence::Race {
        sample: index + 1,
        winner: if second_won { 3 } else { 0 },
    })?;
    if index == 0 {
        let client = if second_won { b } else { a };
        let prepared = run.prepare(client, &winner)?;
        let (status, value) =
            run.create(client, index, Action::WinnerReplay, &prepared, None, false);
        ensure!(
            status == Some(409) && value?.pointer == winner,
            "same-ID replay evidence missing"
        );
        run.evidence(Evidence::Replay {})?;
    }
    Ok(())
}

fn controls(run: &Run<'_>, clients: &mut [GoogleDrive], deadline: Instant) -> Result<()> {
    let frozen = run.frozen;
    let original = frozen.resource(10, false)?.pointer()?;
    let prepared = run.prepare(&mut clients[0], &original)?;
    for control in &mut clients[4..] {
        let expected = "prepared slot application/account mismatch";
        ensure!(
            control
                .observe_prepared_slot(&prepared)
                .err()
                .is_some_and(|err| err.to_string() == expected),
            "changed prepared context was not refused"
        );
    }
    run.evidence(Evidence::ChangedContext {})?;
    let (status, result) = run.create(&mut clients[0], 10, Action::Original, &prepared, None, true);
    ensure!(
        status.is_some_and(success)
            && result
                .err()
                .is_some_and(|err| err.to_string() == "qualification acknowledgement interrupted"),
        "lost acknowledgement boundary unresolved"
    );
    clients[0] = client(run.dir, &frozen.plan, 0, deadline)?;
    let observed = clients[0].observe_prepared_slot(&prepared)?;
    ensure!(
        observed.pointer == original && observed.candidate_match == PointerMatch::Same,
        "original slot not recovered"
    );
    run.evidence(Evidence::LostAcknowledgement {})?;

    for origin in 0..3 {
        let pointer = create_unique(run, &mut clients[origin], 11 + origin)?;
        for (reader, client) in clients[..3].iter_mut().enumerate() {
            let exact = agreed(frozen, client, &pointer).is_ok();
            run.evidence(Evidence::Matrix {
                origin,
                reader,
                exact,
            })?;
            ensure!(
                exact,
                "registration did not observe exact private pointer and descriptor"
            );
        }
    }

    let client = &mut clients[0];
    let absent = frozen.resource(25, false)?.pointer()?;
    ensure!(
        matches!(
            client.observe_agreed_slot(&absent.binding)?,
            AgreedSlotObservation::Absent
        ),
        "unused slot was occupied"
    );
    for (index, expected) in [
        (14, SlotInvalid::Metadata),
        (15, SlotInvalid::Descriptor),
        (16, SlotInvalid::Descriptor),
    ] {
        run.stage(client, index)?;
        let pointer = frozen.resource(index, false)?.pointer()?;
        ensure!(
            matches!(client.observe_agreed_slot(&pointer.binding)?, AgreedSlotObservation::Invalid(reason) if reason == expected),
            "negative slot was not refused"
        );
        run.evidence(Evidence::Negative { index })?;
    }
    let unrelated = create_unique(run, client, 17)?;
    let mut wrong = unrelated.binding.clone();
    wrong.scope_sha256 = frozen.scope("race");
    ensure!(
        matches!(
            client.observe_agreed_slot(&wrong)?,
            AgreedSlotObservation::Invalid(SlotInvalid::Metadata)
        ),
        "cross-scope pointer accepted"
    );
    agreed(frozen, client, &unrelated)?;
    agreed(frozen, client, &original)?;
    run.evidence(Evidence::UnrelatedScope {})?;
    Ok(())
}

fn execute_run(dir: &Directory, approved: &str) -> Result<()> {
    let (frozen, sha) = dir.frozen()?;
    frozen.plan.verify_build()?;
    let (journal, _) = dir.journal()?;
    ensure!(sha == approved, "frozen run identity mismatch");
    journal.admit_run()?;
    let deadline = ledger::deadline(frozen.plan.expires_unix)?;
    let mut clients = (0..ROLES.len())
        .map(|role| client(dir, &frozen.plan, role, deadline))
        .collect::<Result<Vec<_>>>()?;
    let run = Run {
        dir,
        frozen: &frozen,
        journal: Mutex::new(journal),
    };
    run.update(|j| {
        j.started = true;
        Ok(())
    })?;
    qualify_clients(&run, &mut clients)?;
    for index in 18..25 {
        run.stage(&mut clients[Frozen::owner(index, false)], index)?;
    }
    for index in 0..10 {
        race(&run, &mut clients, index, deadline)?;
    }
    controls(&run, &mut clients, deadline)?;
    ensure!(Instant::now() < deadline, "qualification deadline elapsed");
    run.update(|j| {
        j.passed = true;
        Ok(())
    })
}

fn follow_up(dir: &Directory, approved: &str, until: u64, cleanup: Option<&str>) -> Result<()> {
    let (frozen, sha) = dir.frozen()?;
    frozen.plan.verify_build()?;
    let (mut journal, journal_sha) = dir.journal()?;
    ensure!(
        sha == approved && journal.started,
        "frozen run identity mismatch"
    );
    let deadline = ledger::deadline(until)?;
    if let Some(expected) = cleanup {
        ensure!(
            journal_sha == expected && !journal.cleanup_started,
            "cleanup identity mismatch or already consumed"
        );
        journal.cleanup_started = true;
        dir.save("journal.json", &journal)?;
    }
    let mut unresolved = false;
    let mut pointer_unresolved = false;
    for index in 0..25 {
        if journal.removed.contains(&index)
            || !journal
                .sends
                .iter()
                .any(|row| row.index == index && !row.canceled_before_send)
        {
            continue;
        }
        if cleanup.is_some() && index >= 18 && pointer_unresolved {
            unresolved = true;
            continue;
        }
        let check: Result<Option<Action>> = (|| {
            for contender in [false, true] {
                let action = if contender {
                    Action::Contender
                } else {
                    Action::Original
                };
                if !journal.sends.iter().any(|row| {
                    row.index == index && row.action == action && !row.canceled_before_send
                }) {
                    continue;
                }
                let mut client =
                    client(dir, &frozen.plan, Frozen::owner(index, contender), deadline)?;
                let resource = frozen.resource(index, contender)?;
                if wire::presence(&mut client, &resource)? == wire::Presence::Exact {
                    let verified =
                        cleanup.is_none() || wire::delete_verified(&mut client, &resource)?;
                    return Ok(verified.then_some(action));
                }
            }
            Ok(None)
        })();
        let verified = matches!(check, Ok(Some(_)));
        if let Ok(Some(action)) = check {
            journal.observed.insert(index, action);
            if cleanup.is_some() {
                journal.removed.insert(index);
            }
            dir.save("journal.json", &journal)?;
        }
        println!(
            "resource {index} {}",
            if verified { "verified" } else { "unresolved" }
        );
        unresolved |= !verified;
        pointer_unresolved |= index < 18 && !verified;
    }
    ensure!(!unresolved, "qualification evidence incomplete");
    Ok(())
}

fn report(dir: &Directory) -> Result<()> {
    if !dir.exists("frozen.json") && dir.exists("allocation.json") {
        let (ids, _): (Vec<String>, _) = dir.load("allocation.json")?;
        ensure!(ids.len() <= IDS, "invalid qualification allocation");
        println!("allocation incomplete; {} IDs recorded", ids.len());
        return Ok(());
    }
    let (frozen, sha) = dir.frozen()?;
    let (journal, journal_sha) = dir.journal()?;
    println!(
        "buddy_revision {} docs_revision {}",
        frozen.plan.buddy_revision, frozen.plan.docs_revision
    );
    println!("frozen_sha256 {sha} journal_sha256 {journal_sha}");
    println!(
        "started={} passed={} cleanup_started={} observed={} removed={}",
        journal.started,
        journal.passed,
        journal.cleanup_started,
        journal.observed.len(),
        journal.removed.len()
    );
    for send in journal.sends {
        println!(
            "resource {} send {} bytes={} {}",
            send.index,
            send.action.label(),
            send.bytes,
            send.response.map_or_else(
                || if send.canceled_before_send {
                    "canceled-before-send".into()
                } else if send.returned {
                    "no-response".into()
                } else {
                    "unconfirmed".into()
                },
                |status| status.to_string()
            )
        );
    }
    for event in journal.evidence {
        println!("{}", event.label());
    }
    Ok(())
}

fn dispatch(command: Option<Command>) -> Result<()> {
    match command {
        None => {
            println!("No live action selected. Use --help for explicit commands.");
            Ok(())
        }
        Some(Command::Allocate {
            directory,
            plan_sha256,
        }) => allocate(&directory, &plan_sha256),
        Some(Command::Run {
            directory,
            frozen_sha256,
        }) => execute_run(&Directory::open(&directory)?, &frozen_sha256),
        Some(Command::Report { directory }) => report(&Directory::open(&directory)?),
        Some(Command::Observe {
            directory,
            frozen_sha256,
            until_unix,
        }) => follow_up(
            &Directory::open(&directory)?,
            &frozen_sha256,
            until_unix,
            None,
        ),
        Some(Command::Cleanup {
            directory,
            frozen_sha256,
            journal_sha256,
            until_unix,
        }) => follow_up(
            &Directory::open(&directory)?,
            &frozen_sha256,
            until_unix,
            Some(&journal_sha256),
        ),
    }
}

fn main() -> std::process::ExitCode {
    // Avoid parser errors that echo arbitrary argument values. Runtime errors are
    // deliberately summarized because provider errors can carry private context.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) if error.kind() == clap::error::ErrorKind::DisplayHelp => {
            let _ = error.print();
            return std::process::ExitCode::SUCCESS;
        }
        Err(_) => {
            eprintln!("Invalid qualification arguments. Use --help.");
            return std::process::ExitCode::FAILURE;
        }
    };
    if dispatch(cli.command).is_err() {
        eprintln!("Qualification refused or incomplete. Preserve the private ledger.");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_dispatch_and_bounds_are_offline() {
        dispatch(Cli::try_parse_from(["qualification"]).unwrap().command).unwrap();
        assert!(Cli::try_parse_from(["qualification", "run", "example"]).is_err());
        assert!(ledger::deadline(0).is_err());
    }

    #[test]
    fn rendezvous_expires_and_abort_releases_peer() {
        let expired = Gate::new(Instant::now());
        assert!(expired.wait().is_err());
        let gate = Gate::new(Instant::now() + Duration::from_secs(10));
        thread::scope(|scope| {
            let peer = scope.spawn(|| gate.wait());
            gate.abort();
            assert!(peer.join().unwrap().is_err());
        });
    }
}
