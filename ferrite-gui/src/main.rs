//! The Ferrite desktop application. Not yet implemented — the real GUI
//! (process picker, scan panel, results table) lands starting at milestone
//! M1, once `ferrite-core`'s scan engine exists for it to drive.
//!
//! For now, this prints the running process list as proof the ferrite-gui
//! -> ferrite-core crate boundary actually works end to end.

fn main() {
    println!("Ferrite is pre-alpha and not yet functional. See the project README.");
    println!();
    println!("Running processes (via ferrite-core):");

    let mut processes = ferrite_core::list_processes();
    processes.sort_by_key(|p| p.pid);
    for process in processes {
        println!("  {:>7}  {}", process.pid, process.name);
    }
}
