use std::process::Command;

pub fn run_command(cmd: &[String]) -> Vec<String> {
    let Some((program, args)) = cmd.split_first() else {
        return Vec::new();
    };
    let Ok(out) = Command::new(program).args(args).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}
