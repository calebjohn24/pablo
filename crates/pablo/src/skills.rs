use std::{
    ffi::OsString,
    io::{self, Write},
    process::ExitCode,
    time::{Duration, Instant},
};
pub fn inspect(mut args: Vec<OsString>) -> Result<ExitCode, String> {
    if args.is_empty() {
        return Err("usage: pablo skills list|show NAME --config PATH".into());
    }
    let command = args.remove(0);
    let name = if command == "show" {
        if args.is_empty() {
            return Err("missing qualified Skill name".into());
        }
        Some(
            args.remove(0)
                .into_string()
                .map_err(|_| "invalid Skill name")?,
        )
    } else if command == "list" {
        None
    } else {
        return Err("unknown Skill inspection command".into());
    };
    let bootstrap = crate::deployment::Bootstrap::extract(&mut args)?
        .ok_or("Skill inspection requires explicit --config")?;
    if !args.is_empty() {
        return Err("unexpected Skill inspection argument".into());
    }
    let resolved = bootstrap.resolve()?;
    let roots = resolved.skill_roots(None).map_err(|e| e.to_string())?;
    let catalog = pablo_core::skills::discover(
        &roots,
        &pablo_core::CancellationToken::new(),
        Instant::now() + Duration::from_secs(5),
    )
    .map_err(|e| e.to_string())?;
    let output = if let Some(name) = name {
        serde_json::to_vec(catalog.resolve(&name).map_err(|e| e.to_string())?)
    } else {
        serde_json::to_vec(&catalog)
    }
    .map_err(|_| "cannot encode Skill catalog")?;
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(&output)
        .and_then(|_| stdout.write_all(b"\n"))
        .map_err(|_| "cannot write Skill catalog")?;
    Ok(
        if catalog
            .diagnostics
            .iter()
            .any(|d| d.code != pablo_core::skills::Error::Ambiguous)
        {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        },
    )
}
