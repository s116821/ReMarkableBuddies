## ADDED Requirements

### Requirement: Deterministic hold timing
The hold timer SHALL be isolated from the Linux polling loop so contact loss, leaving/reentering the corner and stationary timeout behavior can be tested without device access. The two-second threshold and slot-zero event routing SHALL remain unchanged. Source: src/device/touch.rs.

#### Scenario: Interrupted hold
- **WHEN** contact exits the zone or ends before timeout
- **THEN** its elapsed time is discarded and the next valid hold starts fresh.

#### Scenario: No movement required
- **WHEN** a valid contact stays stationary until the threshold
- **THEN** polling the timer activates exactly at two seconds without new position events.
