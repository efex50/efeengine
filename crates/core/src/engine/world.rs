

#[derive(Debug)]
pub struct GameObject {
    pub position: [f32; 2],
    pub rotation: f32,
}

pub struct EngineWorld{
    pub objects: Vec<GameObject>
}

impl EngineWorld {
    pub fn new() -> Self {
        Self {
            objects: Vec::new(),
        }
    }

    pub fn add_object(&mut self, position: [f32; 2]) {
        self.objects.push(GameObject {
            position,
            rotation: 0.0,
        });
    }
}


#[cfg(test)]
mod tests{
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
        // Random access is simple and safe
        println!("size of entity:{}",size_of::<Entity>());
        println!("{:?}",world.get::<&i32>(a).unwrap());
        println!("{:?}",world.get::<&i32>(b).unwrap());
    }
}
