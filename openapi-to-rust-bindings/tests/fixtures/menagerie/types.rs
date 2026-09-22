pub struct AnimalRequest {
    pub animals: Vec<AnimalUnion>,
    pub model: String,
    pub energy: Option<i64>,
}
pub enum AnimalUnion { Cat(Cat), Dog(Dog) }
pub struct Cat { pub content: CatContent, pub kind: Option<String> }
pub enum CatContent { String(String), Parts(Vec<String>) }
pub struct Dog { pub content: DogContent, pub kind: Option<String> }
pub enum DogContent { String(String), Parts(Vec<String>) }
pub struct AnimalResponse { pub id: String }
