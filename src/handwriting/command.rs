//! Early offline command path; no model, credentials, sync worker or device startup.
use super::*;
use crate::{config::Config, storage::Store};
use std::{io::Read, path::PathBuf, sync::Arc};

pub fn run(config_path: Option<PathBuf>, input: &mut impl Read) -> Report {
    let request = match read_request(input) {
        Ok(v) => v,
        Err(error) => {
            let mut r = Report::new(None);
            r.fail(error);
            return r;
        }
    };
    let mut refusal = Report::new(Some(&request));
    let input = match validate_request(request) {
        Ok(v) => v,
        Err(error) => {
            refusal.fail(error);
            return refusal;
        }
    };
    let outcome = (|| -> Result<LocalCorrections> {
        let config = Config::load(config_path).map_err(|_| Failure::StoreUnavailable)?;
        if config.sync.enabled {
            return Err(Failure::UnsupportedMode);
        }
        let store = Store::open(config.paths.clone()).map_err(|error| {
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::WouldBlock)
            {
                Failure::OwnershipBusy
            } else {
                Failure::StoreUnavailable
            }
        })?;
        LocalCorrections::new(Arc::new(store), &config)
    })();
    match outcome {
        Ok(provider) => provider.execute(input),
        Err(error) => {
            refusal.fail(error);
            refusal
        }
    }
}
