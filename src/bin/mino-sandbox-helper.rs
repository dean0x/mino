//! Mino sandbox helper - macOS privileged helper binary
//!
//! This binary is installed to /usr/local/bin/ during `mino setup --native`
//! and is called via sudoers.d to perform privileged sandbox operations:
//! - Creating/deleting the sandbox system user
//! - Managing pf (packet filter) firewall rules
//! - Setting up filesystem jails
//!
//! Real implementation comes in a later phase.

fn main() {
    eprintln!("mino-sandbox-helper: not yet implemented");
    std::process::exit(1);
}
