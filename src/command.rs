use std::mem::DropGuard;
use std::path::Path;
use std::{env, fmt, fs, io, thread};

use alpm_srcinfo::SourceInfoV1;
use anyhow::{Result as AnyResult, anyhow};
use chrono::{DateTime, Local, Utc};

use crate::{aur, review, system};

#[derive(Debug)]
struct PrintablePackage {
    name: String,
    version: String,
    is_installed: bool,
    votes: i64,
    last_modified_date: DateTime<Utc>,
    available_version: Option<String>,
}

impl PrintablePackage {
    fn from_search_result(
        cache_directory: &str,
        result: &aur::SearchResult,
        aur_client: &mut aur::Client,
    ) -> AnyResult<Self> {
        let is_installed = aur::is_package_installed(&result.name)?;
        if is_installed {
            return Self::from_installed_package(
                &InstalledPackage::from_package_name(cache_directory, &result.name)?,
                aur_client,
            );
        }
        Ok(Self {
            votes: result.num_votes,
            name: result.name.clone(),
            version: result.version.clone(),
            last_modified_date: DateTime::from_timestamp_secs(result.last_modified)
                .ok_or(anyhow!("failed converting to date time"))?,
            is_installed,
            available_version: None,
        })
    }

    fn from_installed_package(
        package: &InstalledPackage,
        aur_client: &mut aur::Client,
    ) -> AnyResult<Self> {
        let aur_info = aur_client.info_package(&package.name)?;
        Ok(Self {
            votes: aur_info.num_votes,
            name: package.name.clone(),
            version: package.info.base.version.to_string(),
            last_modified_date: DateTime::from_timestamp_secs(aur_info.last_modified)
                .ok_or(anyhow!("failed converting to date time"))?,
            is_installed: true,
            available_version: Some(aur_info.version.clone()),
        })
    }
}

impl fmt::Display for PrintablePackage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let last_update_duration = Local::now()
            .to_utc()
            .signed_duration_since(self.last_modified_date);

        let last_update_duration_string = match () {
            () if last_update_duration.num_hours() <= 24 => {
                format!("{}h", last_update_duration.num_hours())
            }
            () if last_update_duration.num_days() <= 7 => {
                format!("{}d", last_update_duration.num_days())
            }
            _ => format!("{}w", last_update_duration.num_weeks()),
        };
        write!(
            f,
            "{}{} {} {{{}}} {} {}",
            match self.is_installed {
                true => "(Installed) ",
                false => "",
            },
            &self.name,
            match &self.available_version {
                Some(available_version) if available_version != &self.version => {
                    format!("{} -> {}", &self.version, &available_version)
                }
                Some(_) => {
                    format!("{} UP TO DATE", &self.version)
                }
                None => {
                    format!("REMOTE {}", &self.version)
                }
            },
            self.votes,
            self.last_modified_date.format("%Y-%m-%d"),
            match last_update_duration_string.as_ref() {
                "" => "".to_string(),
                v => format!("[{}]", v),
            },
        )
    }
}

fn get_cache_directory() -> AnyResult<String> {
    let cache_directory =
        env::var("XDG_CACHE_HOME").unwrap_or(format!("{}/.cache", env::var("HOME")?));
    let cache_directory = format!("{}/taurus", &cache_directory);
    fs::create_dir_all(&cache_directory)?;
    Ok(cache_directory)
}

#[derive(Debug)]
struct Package {
    name: String,
    package_directory: String,
}

impl Package {
    fn new(name: String, cache_directory: &str) -> AnyResult<Self> {
        Ok(Self {
            package_directory: format!("{}/{}", cache_directory, &name),
            name,
        })
    }

    fn git_clone(&self) -> AnyResult<()> {
        let clone_url = format!("https://aur.archlinux.org/{}.git", &self.name);
        system::run_null(None, ["git", "clone", &clone_url, &self.package_directory])?;
        Ok(())
    }

    fn is_installed(&self) -> AnyResult<bool> {
        Ok(aur::is_package_installed(&self.name)?)
    }

    fn makepkg_install(&self) -> AnyResult<()> {
        println!("Running makepkg");
        system::run_inherit(
            Some(&self.package_directory),
            [
                "makepkg",
                "--clean",
                "--cleanbuild",
                "--install",
                "--syncdeps",
                "--force",
            ],
        )?;
        Ok(())
    }

    fn git_fetch(&self) -> AnyResult<()> {
        system::run_null(Some(&self.package_directory), ["git", "fetch"])?;
        Ok(())
    }

    fn git_diff_remote(&self) -> AnyResult<()> {
        thread::scope(|s| {
            let (reader, writer) = io::pipe()?;

            let git_worker = s.spawn(|| {
                system::run_writer(
                    Some(&self.package_directory),
                    ["git", "diff", "@", "@{u}"],
                    writer,
                )
            });
            review::run_pager_git(&self.package_directory, reader)?;

            git_worker.join().expect("no panic")?;
            anyhow::Ok(())
        })?;
        Ok(())
    }

    fn git_reset_hard(&self, head: &str) -> AnyResult<()> {
        system::run_null(
            Some(&self.package_directory),
            ["git", "reset", "--hard", head],
        )?;
        Ok(())
    }

    /// Returns head before the operation
    fn git_reset_hard_remote(&self) -> AnyResult<String> {
        let mut sha_stdout = Vec::new();
        system::run_writer(
            Some(&self.package_directory),
            ["git", "rev-parse", "@"],
            &mut sha_stdout,
        )?;
        self.git_reset_hard("@{u}")?;
        Ok(String::from_utf8(sha_stdout)?.trim().to_string())
    }

    fn update(&self, aur_client: &mut aur::Client) -> AnyResult<()> {
        let installed_package = InstalledPackage::from_dir_entry(&self.package_directory)?;
        if installed_package.is_up_to_date(aur_client)? {
            println!("Package {} is up to date", &self.name);
            return Ok(());
        }

        println!("Fetching git remote");
        self.git_fetch()?;

        self.git_diff_remote()?;
        review::ask_for_permission("install")?;

        let revert_head = self.git_reset_hard_remote()?;
        let revert_guard = DropGuard::new(revert_head, |revert_head| {
            if let Err(v) = self.git_reset_hard(&revert_head) {
                println!("Failed reverting git during abort {}", v);
            }
        });
        self.makepkg_install()?;
        DropGuard::dismiss(revert_guard);

        Ok(())
    }

    fn git_diff_all(&self) -> AnyResult<()> {
        thread::scope(|s| {
            let (reader, writer) = io::pipe()?;

            let git_worker =
                s.spawn(|| review::run_git_repo_diff_all(&self.package_directory, writer));
            review::run_pager_git(&self.package_directory, reader)?;

            git_worker.join().expect("no panic")?;
            anyhow::Ok(())
        })?;
        Ok(())
    }

    fn install_or_update(&self, aur_client: &mut aur::Client) -> AnyResult<()> {
        if !Path::new(&self.package_directory).exists() {
            println!("Cloning {}", &self.name);
            self.git_clone()?;
        }

        if !self.is_installed()? {
            println!("Printing repository files");
            self.git_diff_all()?;
            review::ask_for_permission("install")?;
            self.makepkg_install()?;
            return Ok(());
        }

        self.update(aur_client)?;
        Ok(())
    }
}

#[derive(Debug)]
struct InstalledPackages {
    packages: Vec<InstalledPackage>,
}

impl InstalledPackages {
    fn new(base: &str) -> AnyResult<Self> {
        let mut packages = Vec::new();
        for item in fs::read_dir(base)? {
            let item = item?;

            let info = SourceInfoV1::from_file(&item.path().join(".SRCINFO"))?;
            if !aur::is_package_installed(&info.base.name.to_string())? {
                fs::remove_dir_all(item.path())?;
                continue;
            }

            packages.push(InstalledPackage::from_dir_entry(
                item.path()
                    .to_str()
                    .ok_or(anyhow!("path expected to exist"))?,
            )?);
        }

        Ok(Self { packages })
    }
}

#[derive(Debug)]
struct InstalledPackage {
    directory: String,
    name: String,
    info: SourceInfoV1,
}

impl InstalledPackage {
    fn from_package_name(cache_directory: &str, package: &str) -> AnyResult<Self> {
        Self::from_dir_entry(&format!("{}/{}", cache_directory, package))
    }

    fn from_dir_entry(dir: &str) -> AnyResult<Self> {
        let path = Path::new(dir);

        let info = SourceInfoV1::from_file(&path.join(".SRCINFO"))?;
        if !aur::is_package_installed(&info.base.name.to_string())? {
            return Err(anyhow!(
                "expected version from cache is not installed for {}",
                info.base.name
            ));
        }

        let name = path
            .file_name()
            .ok_or(anyhow!("dir expected to have filename"))?
            .to_str()
            .ok_or(anyhow!("filename is not valid unicode"))?
            .to_string();

        let directory = path
            .to_str()
            .ok_or(anyhow!("path is not valid unicode"))?
            .to_string();

        Ok(Self::new(directory, name, info))
    }

    fn pacman_version(&self) -> AnyResult<String> {
        let mut pacman_output = Vec::new();
        system::run_writer(
            Some(&self.directory),
            ["pacman", "-Q", &self.name],
            &mut pacman_output,
        )?;
        let pacman_output = String::from_utf8(pacman_output)?.trim().to_string();
        for line in pacman_output.lines() {
            let mut iter = line.split(" ");
            let Some(name) = iter.next().map(|v| v.trim()) else {
                continue;
            };
            let Some(version) = iter.next().map(|v| v.trim()) else {
                continue;
            };
            if name == self.name {
                return Ok(version.to_string());
            }
        }

        Err(anyhow!("could not find pacman version for {}", &self.name))
    }

    fn aur_version(&self, aur_client: &mut aur::Client) -> AnyResult<String> {
        let info = aur_client.info_package(&self.name)?;
        Ok(info.version.clone())
    }

    fn is_up_to_date(&self, aur_client: &mut aur::Client) -> AnyResult<bool> {
        let version = self.pacman_version()?;
        if self.info.base.version.to_string() != version {
            return Ok(false);
        }

        Ok(self.aur_version(aur_client)? == version)
    }

    fn to_package(&self, cache_directory: &str) -> AnyResult<Package> {
        Package::new(self.name.clone(), cache_directory)
    }

    fn new(directory: String, name: String, info: SourceInfoV1) -> Self {
        Self {
            directory,
            name,
            info,
        }
    }
}

pub fn command_info_list_installed(aur_client: &mut aur::Client) -> AnyResult<()> {
    let cache_directory = get_cache_directory()?;

    println!("Getting installed aur packages");
    let installed_packages = InstalledPackages::new(&cache_directory)?;

    if installed_packages.packages.is_empty() {
        println!("No packages are tracked, install some!");
    }

    for package in &installed_packages.packages {
        let printable = PrintablePackage::from_installed_package(package, aur_client)?;
        println!("{}", printable);
    }

    Ok(())
}

pub fn command_info_package(
    cache_directory: &str,
    package: &str,
    aur_client: &mut aur::Client,
) -> AnyResult<()> {
    println!("Searching for {}", package);
    let mut info = aur_client.search(package)?;
    info.results.sort_by_key(|v| v.num_votes);

    for result in &info.results {
        let printable = PrintablePackage::from_search_result(cache_directory, result, aur_client)?;
        println!("{}", printable);
    }

    Ok(())
}

pub fn command_info(package: Option<&str>) -> AnyResult<()> {
    let mut aur_client = aur::Client::new();
    let cache_directory = get_cache_directory()?;

    match package {
        Some(v) => command_info_package(&cache_directory, v, &mut aur_client),
        None => command_info_list_installed(&mut aur_client),
    }
}

pub fn command_install_update_all(
    aur_client: &mut aur::Client,
    cache_directory: &str,
) -> AnyResult<()> {
    println!("Getting installed packages");
    let installed_packages = InstalledPackages::new(&cache_directory)?;
    let packages: Vec<AnyResult<Package>> = installed_packages
        .packages
        .iter()
        .map(|v| v.to_package(cache_directory))
        .collect();

    for package in packages {
        let package = package?;
        println!("Updating {}", &package.name);
        package.install_or_update(aur_client)?;
    }

    Ok(())
}

pub fn command_install(package: Option<&str>) -> AnyResult<()> {
    let cache_directory = get_cache_directory()?;
    let mut aur_client = aur::Client::new();

    let Some(package) = package else {
        return command_install_update_all(&mut aur_client, &cache_directory);
    };

    let package = Package::new(package.to_string(), &cache_directory)?;

    package.install_or_update(&mut aur_client)?;
    Ok(())
}
