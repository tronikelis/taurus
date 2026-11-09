use std::env;
use std::fs;
use std::io;
use std::path::Path;
use std::thread;

use anyhow::{Result as AnyResult, anyhow};

use crate::aur;
use crate::review;
use crate::system;

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
    info: aur::InfoResult,
    package_directory: String,
}

impl Package {
    fn new(name: String, aur_client: &mut aur::Client, cache_directory: &str) -> AnyResult<Self> {
        let info = aur_client.info_package(&name)?;

        Ok(Self {
            package_directory: format!("{}/{}", cache_directory, &name),
            name,
            info,
        })
    }

    fn git_clone(&self) -> AnyResult<()> {
        let clone_url = format!("https://aur.archlinux.org/{}.git", &self.name);
        system::run_null(None, ["git", "clone", &clone_url, &self.package_directory])?;
        Ok(())
    }

    fn is_installed(&self) -> AnyResult<bool> {
        aur::is_package_installed(&self.name)
    }

    fn makepkg_install(&self) -> AnyResult<()> {
        println!("Running makepkg");
        system::run_inherit(
            Some(&self.package_directory),
            ["makepkg", "--clean", "--install", "--syncdeps", "--force"],
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

    fn git_reset_hard_remote(&self) -> AnyResult<()> {
        system::run_null(
            Some(&self.package_directory),
            ["git", "reset", "--hard", "@{u}"],
        )?;
        Ok(())
    }

    fn update(&self, aur_client: &mut aur::Client) -> AnyResult<()> {
        println!("Fetching git remote");
        self.git_fetch()?;

        let installed_package = InstalledPackage::from_dir_entry(&self.package_directory)?;
        if installed_package.is_up_to_date(aur_client)? {
            println!("Package {} is up to date", &self.name);
            return Ok(());
        }

        self.git_diff_remote()?;
        review::ask_for_permission("install")?;

        self.git_reset_hard_remote()?;
        self.makepkg_install()?;

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

            let name = item
                .file_name()
                .to_str()
                .ok_or(anyhow!("filename is not valid unicode"))?
                .to_string();
            if !aur::is_package_installed(&name)? {
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
    info: Option<alpm_srcinfo::SourceInfoV1>,
}

impl InstalledPackage {
    fn from_dir_entry(dir: &str) -> AnyResult<Self> {
        let path = Path::new(dir);

        let name = path
            .file_name()
            .ok_or(anyhow!("dir expected to have filename"))?
            .to_str()
            .ok_or(anyhow!("filename is not valid unicode"))?
            .to_string();
        if !aur::is_package_installed(&name)? {
            return Err(anyhow!("package {} is not installed", &name));
        }

        let directory = path
            .to_str()
            .ok_or(anyhow!("path is not valid unicode"))?
            .to_string();

        let mut info = None;
        if path.join(".SRCINFO").exists() {
            info = Some(alpm_srcinfo::SourceInfoV1::from_file(
                &path.join(".SRCINFO"),
            )?);
        }

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
        let pacman_version = pacman_output
            .split(" ")
            .nth(1)
            .ok_or(anyhow!("failed parsing pacman -Q command"))?
            .trim();
        Ok(pacman_version.to_string())
    }

    fn is_up_to_date(&self, aur_client: &mut aur::Client) -> AnyResult<bool> {
        let version = self.pacman_version()?;
        if let Some(info) = &self.info {
            if info.base.version.to_string() != version {
                return Ok(false);
            }
        }

        let info = aur_client.info_package(&self.name)?;

        Ok(info.version == version)
    }

    fn new(directory: String, name: String, info: Option<alpm_srcinfo::SourceInfoV1>) -> Self {
        Self {
            directory,
            name,
            info,
        }
    }
}

pub fn command_info_list_installed() -> AnyResult<()> {
    let cache_directory = get_cache_directory()?;

    println!("Getting installed aur packages");
    let installed_packages = InstalledPackages::new(&cache_directory)?;

    if installed_packages.packages.is_empty() {
        println!("No packages are tracked, install some!");
    }

    for package in &installed_packages.packages {
        let Some(info) = &package.info else {
            println!("{} ???", &package.name);
            continue;
        };
        println!("{} {}", &info.base.name, &info.base.version.to_string());
    }

    Ok(())
}

pub fn command_info_package(package: &str) -> AnyResult<()> {
    let mut aur_client = aur::Client::new();

    println!("Searching for {}", package);
    // todo: replace with search
    let info = aur_client.info([package])?;

    for result in &info.results {
        println!(
            "{} [{}] {} {}",
            &result.name,
            &result.version,
            &result.last_modified,
            &result
                .description
                .as_ref()
                .map(|f| f.as_str())
                .unwrap_or(""),
        );
    }

    Ok(())
}

pub fn command_info(package: Option<&str>) -> AnyResult<()> {
    match package {
        Some(v) => command_info_package(v),
        None => command_info_list_installed(),
    }
}

pub fn command_install(package: Option<&str>) -> AnyResult<()> {
    let package = package.unwrap();

    let cache_directory = get_cache_directory()?;

    let mut aur_client = aur::Client::new();
    let package = Package::new(package.to_string(), &mut aur_client, &cache_directory)?;

    package.install_or_update(&mut aur_client)?;
    Ok(())
}
