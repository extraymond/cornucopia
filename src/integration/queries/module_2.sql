--! authors
SELECT
    *
FROM
    Author;

--! authors_stream
SELECT
    *
FROM
    Author;

--! books
SELECT
    Title
FROM
    Book;

--! books_opt_ret_param ?{1}
SELECT
    Title
FROM
    Book;

--! books_from_author_id(id)
SELECT
    Book.Title
FROM
    BookAuthor
    INNER JOIN Author ON Author.Id = BookAuthor.AuthorId
    INNER JOIN Book ON Book.Id = BookAuthor.BookId
WHERE
    Author.Id = $1;

--! author_name_by_id_opt
SELECT
    Author.Name
FROM
    Author
WHERE
    Author.Id = :id;

--! author_name_by_id
SELECT
    Author.Name
FROM
    Author
WHERE
    Author.Id = :id;

--! author_name_starting_with ?{authorid}
SELECT
    BookAuthor.AuthorId,
    Author.Name,
    BookAuthor.BookId,
    Book.Title
FROM
    BookAuthor
    INNER JOIN Author ON Author.id = BookAuthor.AuthorId
    INNER JOIN Book ON Book.Id = BookAuthor.BookId
WHERE
    Author.Name LIKE CONCAT((:s)::text, '%');

--! return_custom_type
SELECT
    col1
FROM
    CustomTable;

--! select_where_custom_type
SELECT
    col2
FROM
    CustomTable
WHERE (col1).nice = :spongebob_character;

--! select_everything
SELECT
    *
FROM
    Everything;

