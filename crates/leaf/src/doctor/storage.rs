//! Check 5: storage takes a write, a read, a ranged read and a delete.
//!
//! Storage is the configured bucket, or the folder on this machine a
//! `file://` endpoint names; the finding says which. The round trip itself is
//! `leaf_server::validate`'s (the same canary setup runs, plus the ranged
//! read a video player needs): it writes one object under a throwaway key and
//! removes it again whatever became of the reads, and also after a write that
//! got no answer and so may have been stored. This module puts its outcome
//! into the doctor's words.

use leaf_core::config::R2Config;
use leaf_server::setup::Field;
use leaf_server::validate::{DOCTOR_CANARY_KEY, LiveValidator, Step, StorageFailure};

use super::report::{Check, Finding, Redactor};

/// Runs the storage round trip. Live, that is [`LiveValidator`]; tests
/// substitute an outcome.
pub trait StorageProbe: Sync {
    /// Writes, reads, reads a part of and deletes the test object.
    fn round_trip(&self, r2: &R2Config) -> impl Future<Output = Result<(), StorageFailure>> + Send;
}

impl StorageProbe for LiveValidator {
    async fn round_trip(&self, r2: &R2Config) -> Result<(), StorageFailure> {
        self.storage_round_trip(r2).await
    }
}

/// Longest bucket name R2 allows; a longer one is cut when quoted.
const BUCKET_MAX_CHARS: usize = 63;

/// Where the configured storage keeps media, as a finding names it.
enum Place {
    /// A bucket, by its name.
    Bucket(String),
    /// A folder on this machine, by its path.
    Folder(String),
}

impl Place {
    /// Reads the place out of the storage settings, fit to print.
    fn of(r2: &R2Config, redactor: &Redactor) -> Self {
        if !leaf_core::media::is_local_endpoint(&r2.endpoint) {
            return Self::Bucket(redactor.quoted_up_to(&r2.bucket, BUCKET_MAX_CHARS));
        }
        // A `file:` endpoint that names no folder is shown as written: the
        // round trip says what is wrong with it.
        let path = leaf_core::media::local_store_dir(&r2.endpoint).map_or_else(
            || r2.endpoint.clone(),
            |folder| folder.display().to_string(),
        );
        Self::Folder(redactor.quoted(&path))
    }

    /// What the test object is called there, and what holds it.
    const fn nouns(&self) -> (&'static str, &'static str) {
        match self {
            Self::Bucket(_) => ("object", "bucket"),
            Self::Folder(_) => ("file", "folder"),
        }
    }
}

/// Check 5.
pub async fn check(probe: &impl StorageProbe, r2: &R2Config, redactor: &Redactor) -> Vec<Finding> {
    let place = Place::of(r2, redactor);
    let finding = match (probe.round_trip(r2).await, &place) {
        (Ok(()), Place::Bucket(bucket)) => Finding::ok(
            Check::Storage,
            format!(
                "Wrote, read back, read a part of and deleted a test object in bucket “{bucket}”."
            ),
        ),
        (Ok(()), Place::Folder(folder)) => Finding::ok(
            Check::Storage,
            format!(
                "Storage is a local folder, “{folder}”: wrote, read back, read a part of and \
                 deleted a test file there. The files are kept only on this machine."
            ),
        ),
        (Err(failure), place) => failed(&failure, place),
    };
    vec![finding]
}

/// A failed round trip: where it stopped, what to change, and whether the
/// test object is still there.
fn failed(failure: &StorageFailure, place: &Place) -> Finding {
    let at = failure.step.map_or_else(
        || "The storage check could not start".to_owned(),
        |step| format!("The storage check failed at its {} step", step.as_str()),
    );
    let (what, next) = match place {
        // The form-level sentence sends the reader to leaf's server log; the
        // doctor's own log line (on stderr) is where this cause is.
        Place::Bucket(bucket) if failure.error.field == Field::Form => (
            format!(
                ". Storage refused it in bucket “{bucket}” for a reason leaf does not recognise."
            ),
            "The warning leaf doctor logged for this run has storage's own answer. Check the \
             endpoint, bucket and keys, then run the doctor again.",
        ),
        Place::Bucket(_) => (
            format!(". {}", failure.error.message),
            "Correct it in the R2 dashboard, or enter new storage details with --reconfigure \
             (guide/01-install.md, “Changing credentials later”), then run the doctor again.",
        ),
        // The sentence was written for the setup page and says “this
        // folder”, so the folder is named first.
        Place::Folder(folder) => (
            format!(" in the local folder “{folder}”. {}", failure.error.message),
            "Fix the folder, or choose other storage with --reconfigure (guide/01-install.md, \
             “Changing credentials later”), then run the doctor again.",
        ),
    };
    let (thing, holder) = place.nouns();
    let left = match (failure.left_behind, failure.step) {
        (false, _) => String::new(),
        // A write that got no answer: whether it was stored is not known.
        (true, Some(Step::Write)) => format!(
            " The write got no answer, so the test {thing} “{DOCTOR_CANARY_KEY}” may be in the \
             {holder} all the same, and removing it failed: delete it by hand if it is there, or \
             run the doctor again once this is fixed (it overwrites and removes it)."
        ),
        (true, _) => format!(
            " The test {thing} “{DOCTOR_CANARY_KEY}” is still in the {holder}: delete it by \
             hand, or run the doctor again once this is fixed (it overwrites and removes it)."
        ),
    };
    Finding::fail(Check::Storage, format!("{at}{what}{left}"), next)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "tests may panic"
    )]

    use std::sync::Mutex;

    use leaf_server::setup::FieldError;

    use super::*;
    use crate::doctor::report::Status;
    use crate::doctor::testing::{SECRETS, assert_no_secrets, config, printed, redactor, statuses};

    /// A probe that answers as told and records what it was asked with.
    struct Told {
        outcome: Mutex<Option<Result<(), StorageFailure>>>,
        asked: Mutex<Vec<R2Config>>,
    }

    impl Told {
        fn new(outcome: Result<(), StorageFailure>) -> Self {
            Self {
                outcome: Mutex::new(Some(outcome)),
                asked: Mutex::default(),
            }
        }
    }

    impl StorageProbe for Told {
        async fn round_trip(&self, r2: &R2Config) -> Result<(), StorageFailure> {
            self.asked.lock().unwrap().push(r2.clone());
            self.outcome.lock().unwrap().take().unwrap()
        }
    }

    fn failure(
        step: Option<Step>,
        field: Field,
        message: &str,
        left_behind: bool,
    ) -> StorageFailure {
        StorageFailure {
            step,
            error: FieldError::new(field, message),
            left_behind,
        }
    }

    #[tokio::test]
    async fn a_round_trip_that_works_is_one_ok_naming_the_bucket() {
        let probe = Told::new(Ok(()));
        let findings = check(&probe, &config().r2, &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Ok]);
        assert!(findings[0].message.contains("bucket “leaf-media”"));
        assert!(findings[0].message.contains("read a part of"));
        // Asked once, with the configured bucket.
        assert_eq!(*probe.asked.lock().unwrap(), [config().r2]);
        assert_no_secrets(&findings);
    }

    /// Storage settings for the folder `/data/media`, with the placeholder
    /// text a config from before setup offered a folder holds.
    fn folder() -> R2Config {
        R2Config {
            endpoint: "file:///data/media".to_owned(),
            bucket: "local".to_owned(),
            access_key_id: "local".to_owned(),
            secret_access_key: "local".to_owned(),
        }
    }

    #[tokio::test]
    async fn a_round_trip_in_a_folder_says_it_is_a_local_folder() {
        let probe = Told::new(Ok(()));
        let findings = check(&probe, &folder(), &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Ok]);
        assert_eq!(
            findings[0].message,
            "Storage is a local folder, “/data/media”: wrote, read back, read a part of and \
             deleted a test file there. The files are kept only on this machine."
        );
        // No bucket is spoken of, placeholder or not.
        assert!(!printed(&findings).contains("bucket"));
        assert_eq!(*probe.asked.lock().unwrap(), [folder()]);
    }

    #[tokio::test]
    async fn a_failure_in_a_folder_names_the_folder_and_what_to_do_about_it() {
        // (failure, what the finding says)
        let cases = [
            (
                failure(
                    Some(Step::Write),
                    Field::StorageFolder,
                    "leaf can't save files in this folder.",
                    false,
                ),
                "The storage check failed at its write step in the local folder “/data/media”. \
                 leaf can't save files in this folder.",
            ),
            (
                failure(
                    None,
                    Field::StorageFolder,
                    "leaf can't create this folder.",
                    false,
                ),
                "The storage check could not start in the local folder “/data/media”. leaf \
                 can't create this folder.",
            ),
            (
                failure(
                    Some(Step::Delete),
                    Field::StorageFolder,
                    "leaf can't remove files there.",
                    true,
                ),
                "The storage check failed at its delete step in the local folder “/data/media”. \
                 leaf can't remove files there. The test file “leaf-doctor-canary” is still in \
                 the folder: delete it by hand, or run the doctor again once this is fixed (it \
                 overwrites and removes it).",
            ),
            (
                failure(
                    Some(Step::Write),
                    Field::StorageFolder,
                    "This folder didn't answer.",
                    true,
                ),
                "The storage check failed at its write step in the local folder “/data/media”. \
                 This folder didn't answer. The write got no answer, so the test file \
                 “leaf-doctor-canary” may be in the folder all the same, and removing it failed: \
                 delete it by hand if it is there, or run the doctor again once this is fixed \
                 (it overwrites and removes it).",
            ),
        ];
        for (failure, says) in cases {
            let findings = check(&Told::new(Err(failure)), &folder(), &redactor()).await;
            assert_eq!(statuses(&findings), [Status::Fail], "{says}");
            assert_eq!(findings[0].message, says);
            let next = findings[0].next.as_deref().unwrap();
            assert!(next.starts_with("Fix the folder, or choose other storage"));
            assert!(next.contains("--reconfigure"), "{next}");
            // Nothing about a bucket, a dashboard or keys.
            let printed = printed(&findings);
            for word in ["bucket", "R2", "dashboard", "keys"] {
                assert!(!printed.contains(word), "{word} in {printed}");
            }
        }
    }

    #[tokio::test]
    async fn each_failure_names_its_step_its_cause_and_the_next_step() {
        // (failure, what the finding says)
        let cases = [
            (
                failure(
                    Some(Step::Write),
                    Field::R2Bucket,
                    "R2 has no bucket named “x”.",
                    false,
                ),
                "failed at its write step. R2 has no bucket named “x”.",
            ),
            (
                failure(
                    Some(Step::Read),
                    Field::R2AccessKeyId,
                    "The token may not read.",
                    false,
                ),
                "failed at its read step. The token may not read.",
            ),
            (
                failure(
                    Some(Step::ReadRange),
                    Field::R2Endpoint,
                    "It would not return a part.",
                    false,
                ),
                "failed at its ranged read step. It would not return a part.",
            ),
            (
                failure(
                    Some(Step::Delete),
                    Field::R2AccessKeyId,
                    "It may not delete.",
                    true,
                ),
                "failed at its delete step. It may not delete.",
            ),
            (
                failure(
                    None,
                    Field::R2Endpoint,
                    "leaf can't use this endpoint.",
                    false,
                ),
                "could not start. leaf can't use this endpoint.",
            ),
        ];
        for (failure, says) in cases {
            let left_behind = failure.left_behind;
            let findings = check(&Told::new(Err(failure)), &config().r2, &redactor()).await;
            assert_eq!(statuses(&findings), [Status::Fail], "{says}");
            assert!(findings[0].message.contains(says), "{findings:?}");
            assert!(
                findings[0]
                    .next
                    .as_deref()
                    .unwrap()
                    .contains("--reconfigure"),
                "{findings:?}"
            );
            assert_eq!(
                findings[0].message.contains("is still in the bucket"),
                left_behind,
                "{findings:?}"
            );
            assert_no_secrets(&findings);
        }
    }

    #[tokio::test]
    async fn an_object_left_behind_is_named_so_it_can_be_removed() {
        let left = failure(
            Some(Step::Read),
            Field::R2AccessKeyId,
            "The token may not read.",
            true,
        );
        let findings = check(&Told::new(Err(left)), &config().r2, &redactor()).await;
        assert!(
            findings[0]
                .message
                .contains("The test object “leaf-doctor-canary” is still in the bucket")
        );
    }

    #[tokio::test]
    async fn a_write_without_an_answer_says_the_object_may_be_there() {
        // Nobody knows whether such a write was stored, so the finding does
        // not claim the object is there.
        let unanswered = failure(
            Some(Step::Write),
            Field::R2Endpoint,
            "R2 didn't finish the check within 25 seconds.",
            true,
        );
        let findings = check(&Told::new(Err(unanswered)), &config().r2, &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Fail]);
        let message = &findings[0].message;
        assert!(
            message.contains("the test object “leaf-doctor-canary” may be in the bucket"),
            "{message}"
        );
        assert!(!message.contains("is still in the bucket"), "{message}");
    }

    #[tokio::test]
    async fn an_unrecognised_refusal_points_at_the_doctors_own_log() {
        let odd = failure(
            Some(Step::Write),
            Field::Form,
            "leaf's logs have the details (docker compose logs leaf).",
            false,
        );
        let findings = check(&Told::new(Err(odd)), &config().r2, &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Fail]);
        // Not the server's log: this run's.
        assert!(!findings[0].message.contains("docker compose logs"));
        assert!(
            findings[0]
                .message
                .contains("a reason leaf does not recognise")
        );
        assert!(
            findings[0]
                .next
                .as_deref()
                .unwrap()
                .contains("leaf doctor logged")
        );
    }

    #[tokio::test]
    async fn a_storage_answer_that_repeats_a_key_is_not_printed() {
        // S3 error bodies can quote the access key id.
        let echo = failure(
            Some(Step::Write),
            Field::R2AccessKeyId,
            "R2 doesn't recognise SECRET_KEYID_CCC signed with SECRET_KEY_DDD.",
            false,
        );
        let findings = check(&Told::new(Err(echo)), &config().r2, &redactor()).await;
        let printed = printed(&findings);
        for secret in SECRETS {
            assert!(!printed.contains(secret), "{secret} in {printed}");
        }
        assert!(printed.contains("<redacted>"));
    }
}
