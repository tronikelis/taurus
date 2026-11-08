use std::env;
use std::fs;
use std::io;
use std::thread;

use anyhow::{Result as AnyResult, anyhow};

use crate::Args;
use crate::aur;
use crate::review;
use crate::system;

fn get_package_info(name: &str) -> AnyResult<aur::InfoResult> {
    let mut aur_client = aur::Client::new();
    let info = aur_client.info([name])?;

    let result = info
        .results
        .get(0)
        .ok_or_else(|| anyhow!("{} not found", name))?;

    Ok(result.clone())
}

pub fn command_info(args: &Args) -> AnyResult<()> {
    println!("Searching for {}", &args.package);
    let result = get_package_info(&args.package)?;

    println!(
        "{} [{}] {} {}",
        result.name,
        result.version,
        result.last_modified,
        result
            .description
            .as_ref()
            .map(|f| f.as_str())
            .unwrap_or(""),
    );

    Ok(())
}

fn makepkg_install(dir: &str) -> AnyResult<()> {
    println!("Running makepkg");
    system::run_inherit(Some(dir), ["makepkg", "--clean", "--install", "--syncdeps"])?;
    Ok(())
}

fn clone_and_install(package: &str, package_directory: &str) -> AnyResult<()> {
    let clone_url = format!("https://aur.archlinux.org/{}.git", package);
    println!("Cloning {}", &clone_url);
    system::run_null(None, ["git", "clone", &clone_url, &package_directory])?;

    if !aur::is_package_installed(package)? {
        println!("Printing repository files");
        thread::scope(|s| {
            let (reader, writer) = io::pipe()?;

            let git_worker = s.spawn(|| review::run_git_repo_diff_all(package_directory, writer));
            review::run_pager_git(package_directory, reader)?;

            git_worker.join().expect("no panic")?;
            anyhow::Ok(())
        })?;

        review::ask_for_permission("install")?;
        makepkg_install(package_directory)?;

        return Ok(());
    }

    update_and_install(package_directory, package)?;
    Ok(())
}

fn update_and_install(package_directory: &str, package: &str) -> AnyResult<()> {
    println!("Fetching git remote");

    system::run_null(Some(package_directory), ["git", "fetch"])?;

    if aur::is_package_up_to_date(package_directory, package)? {
        println!("Package {} is up to date", package);
        return Ok(());
    }

    thread::scope(|s| {
        let (reader, writer) = io::pipe()?;

        let git_worker = s.spawn(|| {
            system::run_writer(
                Some(package_directory),
                ["git", "diff", "@", "@{u}"],
                writer,
            )
        });
        review::run_pager_git(package_directory, reader)?;

        git_worker.join().expect("no panic")?;
        anyhow::Ok(())
    })?;

    review::ask_for_permission("install")?;

    system::run_null(Some(package_directory), ["git", "reset", "--hard", "@{u}"])?;
    makepkg_install(package_directory)?;

    Ok(())
}

pub fn command_install(args: &Args) -> AnyResult<()> {
    println!("Searching for {}", &args.package);
    get_package_info(&args.package)?;

    let cache_directory =
        env::var("XDG_CACHE_HOME").unwrap_or(format!("{}/.cache", env::var("HOME")?));
    let cache_directory = format!("{}/taurus", &cache_directory);
    fs::create_dir_all(&cache_directory)?;

    let package_directory = format!("{}/{}", cache_directory, &args.package);
    if !fs::exists(&package_directory)? {
        return Ok(clone_and_install(&args.package, &package_directory)?);
    }

    update_and_install(&package_directory, &args.package)?;

    Ok(())
}
