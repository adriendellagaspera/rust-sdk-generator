impl HttpClient {
    pub async fn create_book(&self, request: CreateBookRequest) -> Result<BookResponse, Error> { todo!() }
    pub async fn list_books(&self, limit: Option<i64>) -> Result<BookList, Error> { todo!() }
    pub async fn delete_book(&self, book_id: impl AsRef<str>) -> Result<(), Error> { todo!() }
    pub async fn download_book(&self, book_id: impl AsRef<str>) -> Result<futures_util::stream::BoxStream<'static, Result<bytes::Bytes, reqwest::Error>>, Error> { todo!() }
}
