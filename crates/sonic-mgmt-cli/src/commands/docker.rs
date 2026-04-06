//! `sonic-mgmt docker` -- manage the sonic-mgmt test runner container.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
pub struct DockerCmd {
    #[command(subcommand)]
    pub action: DockerAction,
}

#[derive(clap::Subcommand, Debug)]
pub enum DockerAction {
    /// Build the test runner image
    Build {
        #[arg(long)]
        no_cache: bool,
    },
    /// Start the test runner container (detached)
    Up,
    /// Stop and remove the test runner container
    Down,
    /// Show test runner status
    Status,
    /// Open a shell inside the test runner
    Shell,
    /// Run integration tests inside the container against a testbed
    Test {
        #[arg(long)]
        testbed: Option<String>,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

// ---------------------------------------------------------------------------
// Env
// ---------------------------------------------------------------------------

struct Env {
    compose_file: String,
    container: String,
}

impl Default for Env {
    fn default() -> Self {
        Self {
            compose_file: env_or("COMPOSE_FILE", "docker/docker-compose.yml"),
            container: env_or("SONIC_CONTAINER", "sonic-mgmt-runner"),
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn handle(cmd: DockerCmd) -> Result<()> {
    let env = Env::default();
    match cmd.action {
        DockerAction::Build { no_cache } => cmd_build(&env, no_cache),
        DockerAction::Up => cmd_up(&env),
        DockerAction::Down => cmd_down(&env),
        DockerAction::Status => cmd_status(&env),
        DockerAction::Shell => cmd_shell(&env),
        DockerAction::Test { testbed, args } => cmd_test(&env, testbed, &args),
    }
}

// ---------------------------------------------------------------------------
// Logging
// ---------------------------------------------------------------------------

fn timestamp() -> String {
    let now = chrono::Local::now();
    now.format("%H:%M:%S").to_string()
}

fn log_info(msg: &str) {
    eprintln!(
        " {} {} {}",
        style(timestamp()).dim(),
        style("INFO").cyan().bold(),
        msg,
    );
}

fn log_ok(msg: &str) {
    eprintln!(
        " {} {}   {}",
        style(timestamp()).dim(),
        style("OK").green().bold(),
        msg,
    );
}

fn log_fail(msg: &str) {
    eprintln!(
        " {} {} {}",
        style(timestamp()).dim(),
        style("FAIL").red().bold(),
        msg,
    );
}

fn spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template(&format!(
            " {{spinner:.dim}} {} {{msg}}",
            style("WAIT").blue().bold(),
        ))
        .unwrap()
        .tick_strings(&[".", "o", "O", "o", "."]),
    );
    pb.set_message(msg.to_owned());
    pb.enable_steady_tick(Duration::from_millis(120));
    pb
}

// ---------------------------------------------------------------------------
// Shell helpers
// ---------------------------------------------------------------------------

fn run_streaming(cmd: &str, args: &[&str], pb: &ProgressBar) -> Result<()> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn `{cmd}`"))?;

    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
            let t = line.trim();
            if !t.is_empty() {
                let display = if t.len() > 64 { &t[..61] } else { t };
                pb.set_message(display.to_owned());
            }
        }
    }

    let status = child.wait()?;
    if !status.success() {
        bail!("`{cmd}` exited with {status}");
    }
    Ok(())
}

fn run_capture(cmd: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("failed to run `{cmd}`"))?;

    let mut out = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.is_empty() {
        out.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    Ok(out)
}

fn run_quiet(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn container_state(env: &Env) -> String {
    run_capture(
        "docker",
        &["inspect", "-f", "{{.State.Status}}", &env.container],
    )
    .map(|s| s.trim().to_owned())
    .unwrap_or_default()
}

fn image_exists(name: &str) -> bool {
    run_quiet("docker", &["image", "inspect", name])
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

fn cmd_build(env: &Env, no_cache: bool) -> Result<()> {
    log_info("building test runner image");
    let pb = spinner("docker compose build");

    let mut args: Vec<&str> = vec!["compose", "-f", &env.compose_file, "build"];
    if no_cache {
        args.push("--no-cache");
    }

    run_streaming("docker", &args, &pb)?;
    pb.finish_and_clear();
    log_ok("image built");
    Ok(())
}

fn cmd_up(env: &Env) -> Result<()> {
    if container_state(env) == "running" {
        log_ok("already running");
        return Ok(());
    }

    log_info("starting test runner container");
    let pb = spinner("docker compose up --build -d");
    run_streaming(
        "docker",
        &["compose", "-f", &env.compose_file, "up", "-d", "--build"],
        &pb,
    )?;
    pb.finish_and_clear();
    log_ok("container started");
    Ok(())
}

fn cmd_down(env: &Env) -> Result<()> {
    let state = container_state(env);
    if state.is_empty() {
        log_ok("nothing to remove");
    } else {
        log_info("stopping container");
        let pb = spinner("docker compose down");
        run_streaming(
            "docker",
            &["compose", "-f", &env.compose_file, "down"],
            &pb,
        )?;
        pb.finish_and_clear();
        log_ok("container removed");
    }
    Ok(())
}

fn cmd_status(env: &Env) -> Result<()> {
    let state = container_state(env);
    let state_str = if state.is_empty() {
        "not found"
    } else {
        &state
    };
    let state_styled = match state_str {
        "running" => style(state_str).green(),
        "exited" | "dead" => style(state_str).red(),
        "paused" => style(state_str).yellow(),
        _ => style(state_str).dim(),
    };
    eprintln!();
    eprintln!(
        "  {:<14} {}  {}",
        style("container").bold(),
        env.container,
        state_styled
    );

    eprintln!();
    eprintln!("  {}", style("image").bold());
    let img = "sonic-mgmt-runner:latest";
    eprintln!(
        "    {:<32} {}",
        img,
        if image_exists(img) {
            style("loaded").green()
        } else {
            style("absent").dim()
        },
    );

    if state == "running" {
        eprintln!();
        let mounts = run_capture(
            "docker",
            &[
                "inspect",
                "-f",
                "{{range .Mounts}}{{.Source}} -> {{.Destination}}\n{{end}}",
                &env.container,
            ],
        )
        .unwrap_or_default();
        if !mounts.trim().is_empty() {
            eprintln!("  {}", style("volumes").bold());
            for line in mounts.trim().lines() {
                eprintln!("    {}", style(line).dim());
            }
        }
    }

    eprintln!();
    Ok(())
}

fn cmd_shell(env: &Env) -> Result<()> {
    log_info("opening shell in test runner");
    let status = Command::new("docker")
        .args([
            "compose",
            "-f",
            &env.compose_file,
            "run",
            "--rm",
            "sonic-mgmt",
            "bash",
        ])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to launch shell")?;

    if !status.success() {
        bail!("shell exited with {status}");
    }
    Ok(())
}

fn cmd_test(env: &Env, testbed: Option<String>, extra_args: &[String]) -> Result<()> {
    log_info("running integration tests in container");

    let mut args = vec![
        "compose",
        "-f",
        &env.compose_file,
        "run",
        "--rm",
        "--entrypoint",
        "cargo",
    ];

    // Pass testbed env var if provided
    let testbed_env;
    if let Some(ref tb) = testbed {
        testbed_env = format!("SONIC_TESTBED={tb}");
        args.extend_from_slice(&["-e", &testbed_env]);
    }

    args.extend_from_slice(&[
        "sonic-mgmt",
        "test",
        "-p",
        "sonic-integration-tests",
        "--test",
        "integration",
        "--",
        "--ignored",
        "--test-threads=1",
    ]);

    // Append any extra args the user passed
    let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
    args.extend_from_slice(&extra_refs);

    let status = Command::new("docker")
        .args(&args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to run tests")?;

    if !status.success() {
        log_fail("tests failed");
        bail!("test run exited with {status}");
    }
    log_ok("tests passed");
    Ok(())
}
