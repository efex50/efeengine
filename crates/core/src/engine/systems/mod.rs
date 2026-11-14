



#[deprecated]
pub fn add_system(){

}



pub trait GameSystem{
    fn init(&mut self){}
    fn engin_update(&mut self){}
    fn engin_physics_update(&mut self){}
    fn engin_render_update(&mut self){}
    fn engin_exit(&mut self){}
}





#[cfg(test)]
mod tests{
    use std::alloc;

    use hecs::{Entity, World};

    #[test]
    fn hecs_test(){
        let mut world = World::new();
        // Nearly any type can be used as a component with zero boilerplate
        let a: hecs::Entity = world.spawn((123, true, "abc"));
        println!("{:?}",a);
        let b = world.spawn((42, false));
        println!("{:?}",b);
        let b = world.spawn((42, false,"sa".to_string()));
        println!("{:?}",b);
        let b = world.spawn((42, false,"so".to_string()));
        println!("{:?}",b);
        let b = world.spawn((42, false));
        println!("{:?}",b);
        // Systems can be simple for loops
        for (number, &flag,) in world.query_mut::<(&mut i32, &bool)>() {
            println!("sa");
            if !flag { *number *= 2; }
        }
        //world.insert(entity, components)
        ;
        // Random access is simple and safe
        println!("size of entity:{}",size_of::<Entity>());
        println!("{:?}",world.get::<&i32>(a).unwrap());
        println!("{:?}",world.get::<&i32>(b).unwrap());
    }

}