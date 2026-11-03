use anyhow::{Result, bail};
use std::io::{self, IsTerminal, Write};

/// Blocks until the operator types `yes`.
///
/// Only the exact word is accepted: `y` and a bare newline are easy to type by
/// reflex, which is the opposite of what a confirmation is for.
///
/// Synchronous, and called from async code. Nothing else is running on the
/// runtime while we wait for a human, so `spawn_blocking` would only add noise.
pub fn confirm(question: &str) -> Result<()> {
    // Refusing beats prompting when nobody can answer. In CI stdin is a pipe,
    // and a prompt there would hang until the job timed out.
    if !io::stdin().is_terminal() {
        bail!("{question}\n  stdin is not a terminal - pass --yes to proceed without confirming");
    }

    print!("{question}\n  Only `yes` will be accepted.\n\n  Enter a value: ");

    // stdout is line buffered and the prompt has no newline, so without this
    // flush the question would appear after the answer was typed.
    io::stdout().flush()?;

    // No newline of our own: we only read when stdin is a terminal, and the
    // terminal already echoed the one the operator typed.
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;

    if answer.trim() != "yes" {
        bail!("cancelled");
    }

    Ok(())
}
