pub struct CreateBookRequest {
    pub title: String,
    pub pages: Option<i64>,
}
pub struct BookResponse {
    pub id: String,
    pub title: String,
}
pub struct BookList {
    pub items: Vec<BookResponse>,
}
