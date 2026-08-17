//! `fr git setup` — configure this clone for frame.
//!
//! Deliberately does **not** load the project, only discover it. Setup is most
//! useful on a project that is in some way broken, and refusing to fix a
//! `.gitignore` because a track file will not parse would be exactly backwards.

use crate::cli::commands::{GitAction, GitCmd};
use crate::ops::git_setup::{self, SetupReport, StepStatus};

pub fn cmd_git(args: GitCmd, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    match args.action {
        GitAction::Setup(setup_args) => {
            crate::io::dryrun::arm(setup_args.dry_run);
            let root = super::discover_project_root()?;
            let report = git_setup::run(&root, setup_args.dry_run);

            if json {
                print_json(&report, setup_args.dry_run);
            } else {
                print_human(&report, setup_args.dry_run);
            }

            if report.failed() {
                return Err("some steps could not be applied".into());
            }
            Ok(())
        }
    }
}

fn print_human(report: &SetupReport, dry_run: bool) {
    if !report.in_git {
        println!("not a git repository — nothing to configure");
        return;
    }

    for step in &report.steps {
        match &step.status {
            StepStatus::AlreadyCorrect => println!("  ok      {}", step.name),
            StepStatus::Changed => {
                println!(
                    "  {}  {}",
                    if dry_run { "would " } else { "set   " },
                    step.name
                );
                for line in &step.detail {
                    println!("            {line}");
                }
            }
            StepStatus::Failed(why) => println!("  failed  {} — {}", step.name, why),
        }
    }

    if report.fr_not_on_path {
        println!();
        println!("warning: `fr` is not on PATH, so git cannot run the merge driver.");
        println!("         install it, or set merge.frame.driver to an absolute path.");
    }

    println!();
    if dry_run {
        if report.changed_anything() {
            println!("dry run — nothing was written. Re-run without --dry-run to apply.");
        } else {
            println!("already configured — nothing to do.");
        }
    } else if report.changed_anything() {
        println!("configured. Commit .gitignore and .gitattributes;");
        println!("the merge driver is per-clone, so each clone runs `fr git setup` once.");
    } else {
        println!("already configured — nothing to do.");
    }
}

fn print_json(report: &SetupReport, dry_run: bool) {
    let steps: Vec<serde_json::Value> = report
        .steps
        .iter()
        .map(|step| {
            let (status, error) = match &step.status {
                StepStatus::AlreadyCorrect => ("already_correct", None),
                StepStatus::Changed => ("changed", None),
                StepStatus::Failed(why) => ("failed", Some(why.clone())),
            };
            serde_json::json!({
                "name": step.name,
                "status": status,
                "detail": step.detail,
                "error": error,
            })
        })
        .collect();

    let out = serde_json::json!({
        "in_git": report.in_git,
        "dry_run": dry_run,
        "changed": report.changed_anything(),
        "fr_on_path": !report.fr_not_on_path,
        "steps": steps,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}
