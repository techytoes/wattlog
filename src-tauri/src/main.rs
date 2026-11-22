#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::Parser;
use wattlog_lib::cli::{run_cli_if_requested, CliArgs};

fn main() {
    // Parse CLI flags (--demo / --plain / --json) BEFORE booting AppKit / WebKit.
    let args = CliArgs::parse();
    if run_cli_if_requested(&args) {
        return;
    }

    wattlog_lib::run_gui(args.demo);
}
