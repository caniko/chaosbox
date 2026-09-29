mod provider;

use helper::dependency;
use provider::{answer as renamed, Work, Worker};

pub fn consume() -> u32 {
    let value = ("🦀", renamed()).1;
    let reference = renamed;
    let shadowed = {
        let renamed = || 7;
        renamed()
    };
    value + reference() + shadowed + Worker.run() + dependency()
}

#[cfg(feature = "extra")]
pub fn configured() -> u32 {
    renamed()
}

macro_rules! make_generated {
    () => {
        pub fn generated() -> u32 {
            9
        }
    };
}
make_generated!();

pub fn expansion_user() -> u32 {
    generated()
}
