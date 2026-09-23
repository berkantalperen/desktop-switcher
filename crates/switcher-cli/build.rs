//! Gives the Windows executable its icon and name; does nothing elsewhere.

fn main() {
    switcher_icon::build::embed("Desktop Switcher (command line)");
}
