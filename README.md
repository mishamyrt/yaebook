<p align="center">
  <img src="./docs/logo.svg" width="250" alt="yaebook" />
  <br />
  Загрузчик EPUB из <a href="https://books.yandex.ru">Яндекс Книг</a>.
</p>

---

<p align="center">
  <a href="https://github.com/mishamyrt/yaebook/actions/workflows/qa.yaml">
    <img src="https://github.com/mishamyrt/yaebook/actions/workflows/qa.yaml/badge.svg" alt="Quality Assurance" />
  </a>
</p>

Яебук скачивает книги из сервиса Яндекс Книги в формате, подходящем для просмотра на iPad, Kindle и других читалках.

## Получение токена

Нужен токен Яндекса формат: `y0_AgAAAA...`

1. Откройте: https://oauth.yandex.ru/authorize?response_type=token&client_id=4483e97bab6e486a9822973109a14d05
2. Авторизуйтесь
3. Скопируйте весь access_token из URL (начинается с y0_)

## Использование

```bash
./target/release/yaebook --token 'y0_...' \
  --output-dir /path/to/books \
  https://books.yandex.ru/books/HLwwn7ea
```

Вместо `--token` можно передать переменную окружения:

```bash
YA_BOOKS_TOKEN='y0_...' ./target/release/yaebook \
  https://books.yandex.ru/books/HLwwn7ea
```

Папка экспорта задаётся через `-o` или `--output-dir`; по умолчанию используется
текущая рабочая папка.

## Сборка

```bash
cargo build --release
```

## Проверка

```bash
make test
make lint
```
