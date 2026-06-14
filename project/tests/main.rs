use std::fmt::Debug;

trait Base {
    type Item;
    fn get(&self) -> &Self::Item;
}

trait Derived: Base
where
    Self::Item: Debug,
{
    fn print(&self) {
        println!("{:?}", self.get());
    }
}
