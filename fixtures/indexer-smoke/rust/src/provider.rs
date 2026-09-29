pub fn answer() -> u32 {
    42
}

pub trait Work {
    fn run(&self) -> u32;
}

pub struct Worker;

impl Work for Worker {
    fn run(&self) -> u32 {
        answer()
    }
}
