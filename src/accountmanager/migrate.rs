use std::{fs::File, io::Read, path::Path};

use log::debug;
use serde::de::Error;
use steamguard::SteamGuardAccount;
use thiserror::Error;

use super::{
	legacy::{SdaAccount, SdaManifest},
	manifest::ManifestV1,
	EntryEncryptionParams, EntryLoader, Manifest,
};

pub(crate) fn load_and_migrate(
	manifest_path: &Path,
	passkey: Option<&String>,
) -> Result<(Manifest, Vec<SteamGuardAccount>), MigrationError> {
	backup_file(manifest_path)?;
	let parent = manifest_path.parent().unwrap();
	parent.read_dir()?.for_each(|e| {
		let entry = e.unwrap();
		if entry.file_type().unwrap().is_file() {
			let path = entry.path();
			if path.extension().unwrap() == "maFile" {
				backup_file(&path).unwrap();
			}
		}
	});

	do_migrate(manifest_path, passkey)
}

fn do_migrate(
	manifest_path: &Path,
	passkey: Option<&String>,
) -> Result<(Manifest, Vec<SteamGuardAccount>), MigrationError> {
	let mut file = File::open(manifest_path)?;
	let mut buffer = String::new();
	file.read_to_string(&mut buffer)?;
	let mut manifest: MigratingManifest =
		deserialize_manifest(buffer).map_err(MigrationError::ManifestDeserializeFailed)?;

	if manifest.is_encrypted() && passkey.is_none() {
		return Err(MigrationError::MissingPasskey);
	} else if !manifest.is_encrypted() && passkey.is_some() {
		// no custom error because this is an edge case, mostly user error
		return Err(MigrationError::UnexpectedError(anyhow::anyhow!("A passkey was provided but the manifest is not encrypted. Aborting migration because it would encrypt the maFiles, and you probably didn't mean to do that.")));
	}

	let folder = manifest_path.parent().unwrap();
	let mut accounts = manifest.load_all_accounts(folder, passkey)?;

	while !manifest.is_latest() {
		manifest = manifest.upgrade();

		for account in accounts.iter_mut() {
			*account = account.clone().upgrade();
		}
	}

	// HACK: force account names onto manifest entries
	let mut manifest: Manifest = manifest.into();
	let accounts: Vec<SteamGuardAccount> = accounts.into_iter().map(|a| a.into()).collect();
	for (i, entry) in manifest.entries.iter_mut().enumerate() {
		entry.account_name = accounts[i].account_name.to_lowercase();
	}

	Ok((manifest, accounts))
}

fn backup_file(path: &Path) -> anyhow::Result<()> {
	let backup_path = Path::join(
		path.parent().unwrap(),
		format!("{}.bak", path.file_name().unwrap().to_str().unwrap()),
	);
	std::fs::copy(path, backup_path)?;
	Ok(())
}

#[derive(Debug, Error)]
pub(crate) enum MigrationError {
	#[error("Passkey is required to decrypt manifest")]
	MissingPasskey,
	#[error("Failed to deserialize manifest: {0}")]
	ManifestDeserializeFailed(serde_json::Error),
	#[error("IO error when upgrading manifest: {0}")]
	IoError(#[from] std::io::Error),
	#[error("An unexpected error occurred during manifest migration: {0}")]
	UnexpectedError(#[from] anyhow::Error),
}

#[derive(Debug)]
enum MigratingManifest {
	Sda(SdaManifest),
	ManifestV1(ManifestV1),
}

impl MigratingManifest {
	pub fn upgrade(self) -> Self {
		match self {
			Self::Sda(sda) => Self::ManifestV1(sda.into()),
			Self::ManifestV1(_) => self,
		}
	}

	pub fn is_latest(&self) -> bool {
		matches!(self, Self::ManifestV1(_))
	}

	pub fn is_encrypted(&self) -> bool {
		match self {
			Self::Sda(manifest) => manifest.entries.iter().any(|e| e.encryption.is_some()),
			Self::ManifestV1(manifest) => manifest.entries.iter().any(|e| e.encryption.is_some()),
		}
	}

	pub fn load_all_accounts(
		&self,
		folder: &Path,
		passkey: Option<&String>,
	) -> anyhow::Result<Vec<MigratingAccount>> {
		debug!("loading all accounts for migration");
		let accounts = match self {
			Self::Sda(sda) => {
				let (accounts, errors) = sda
					.entries
					.iter()
					.map(|e| {
						let params: Option<EntryEncryptionParams> =
							e.encryption.clone().map(|e| e.into());
						e.load(&Path::join(folder, &e.filename), passkey, params.as_ref())
					})
					.partition::<Vec<_>, _>(Result::is_ok);
				let accounts: Vec<_> = accounts.into_iter().map(Result::unwrap).collect();
				let errors: Vec<_> = errors.into_iter().map(Result::unwrap_err).collect();
				if !errors.is_empty() {
					return Err(anyhow::anyhow!(
						"Failed to load some accounts: {:?}",
						errors
					));
				}
				accounts.into_iter().map(MigratingAccount::Sda).collect()
			}
			Self::ManifestV1(manifest) => {
				let (accounts, errors) = manifest
					.entries
					.iter()
					.map(|e| {
						e.load(
							&Path::join(folder, &e.filename),
							passkey,
							e.encryption.as_ref(),
						)
					})
					.partition::<Vec<_>, _>(Result::is_ok);
				let accounts: Vec<_> = accounts.into_iter().map(Result::unwrap).collect();
				let errors: Vec<_> = errors.into_iter().map(Result::unwrap_err).collect();
				if !errors.is_empty() {
					return Err(anyhow::anyhow!(
						"Failed to load some accounts: {:?}",
						errors
					));
				}
				accounts
					.into_iter()
					.map(MigratingAccount::ManifestV1)
					.collect()
			}
		};
		Ok(accounts)
	}
}

impl From<MigratingManifest> for Manifest {
	fn from(migrating: MigratingManifest) -> Self {
		match migrating {
			MigratingManifest::ManifestV1(manifest) => manifest,
			_ => panic!("Manifest is not at the latest version!"),
		}
	}
}

fn deserialize_manifest(text: String) -> Result<MigratingManifest, serde_json::Error> {
	let json: serde_json::Value = serde_json::from_str(&text)?;
	debug!("deserializing manifest: version {}", json["version"]);
	if json["version"] == 1 {
		let manifest: ManifestV1 = serde_json::from_str(&text)?;
		Ok(MigratingManifest::ManifestV1(manifest))
	} else if json["version"] == serde_json::Value::Null {
		let manifest: SdaManifest = serde_json::from_str(&text)?;
		Ok(MigratingManifest::Sda(manifest))
	} else {
		Err(serde_json::Error::custom(format!(
			"Unknown manifest version: {}",
			json["version"]
		)))
	}
}

#[derive(Debug, Clone)]
enum MigratingAccount {
	Sda(SdaAccount),
	ManifestV1(SteamGuardAccount),
}

impl MigratingAccount {
	pub fn upgrade(self) -> Self {
		match self {
			Self::Sda(sda) => Self::ManifestV1(sda.into()),
			Self::ManifestV1(_) => self,
		}
	}

	pub fn is_latest(&self) -> bool {
		matches!(self, Self::ManifestV1(_))
	}
}

impl From<MigratingAccount> for SteamGuardAccount {
	fn from(migrating: MigratingAccount) -> Self {
		match migrating {
			MigratingAccount::ManifestV1(account) => account,
			_ => panic!("Account is not at the latest version!"),
		}
	}
}

pub fn load_and_upgrade_sda_account(path: &Path) -> anyhow::Result<SteamGuardAccount> {
	let file = File::open(path)?;
	let account: SdaAccount = serde_json::from_reader(file)?;
	let mut account = MigratingAccount::Sda(account);
	while !account.is_latest() {
		account = account.upgrade();
	}

	Ok(account.into())
}

#[cfg(test)]
mod tests {
	use crate::accountmanager::CURRENT_MANIFEST_VERSION;

	use super::*;

	#[test]
	fn should_migrate_to_latest_version() -> anyhow::Result<()> {
		#[derive(Debug)]
		struct Test {
			manifest: &'static str,
			passkey: Option<String>,
		}
		let cases = vec![
			Test {
				manifest: "src/fixtures/maFiles/compat/1-account/manifest.json",
				passkey: None,
			},
			Test {
				manifest: "src/fixtures/maFiles/compat/1-account-encrypted/manifest.json",
				passkey: Some("password".into()),
			},
			Test {
				manifest: "src/fixtures/maFiles/compat/2-account/manifest.json",
				passkey: None,
			},
			Test {
				manifest: "src/fixtures/maFiles/compat/missing-account-name/manifest.json",
				passkey: None,
			},
			Test {
				manifest: "src/fixtures/maFiles/compat/no-webcookie/manifest.json",
				passkey: None,
			},
		];
		for case in cases {
			eprintln!("testing: {:?}", case);
			let (manifest, accounts) = do_migrate(Path::new(case.manifest), case.passkey.as_ref())?;
			assert_eq!(manifest.version, CURRENT_MANIFEST_VERSION);
			assert_eq!(manifest.entries[0].account_name, "example");
			assert_eq!(manifest.entries[0].steam_id, 1234);
			assert_eq!(accounts[0].account_name, "example");
			assert_eq!(accounts[0].steam_id, 1234);
		}
		Ok(())
	}
}