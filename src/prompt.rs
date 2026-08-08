//! Interactive y/n prompts (Section 9 of the spec: the Linux dependency
//! flow asks the user whether to try installing missing dependencies).

use std::io::{BufRead, Write};

use crate::error::{Result, UpkgError};

/// Ask a yes/no question on stdin/stdout. Returns `Ok(true)` for y/yes.
pub fn ask_yes_no(question: &str) -> Result<bool> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    loop {
        write!(stdout, "{question} [y/n] ").map_err(UpkgError::Io)?;
        stdout.flush().map_err(UpkgError::Io)?;
        let mut line = String::new();
        let n = stdin.lock().read_line(&mut line).map_err(UpkgError::Io)?;
        if n == 0 {
            // EOF: treat as "no" but report abort to the caller distinctly.
            return Err(UpkgError::Aborted);
        }
        match line.trim().to_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => {
                eprintln!("please answer y or n");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Interactive prompts are exercised via the tmux-cli agent; nothing to
    // unit test here.
}
