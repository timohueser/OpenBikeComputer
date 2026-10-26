//! `trailview-heights JOB OUT`: the heights of a job file of `tools/trailview/solvers.py`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [job, out] = &args[..] else {
        eprintln!("usage: trailview-heights JOB OUT");
        std::process::exit(2);
    };
    let ran = obc_trailview_features::job::Job::read(job.as_ref()).and_then(|job| job.run(out.as_ref()));
    if let Err(error) = ran {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
