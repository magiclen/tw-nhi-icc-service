TW NHI Card Service
===============

[![CI](https://github.com/magiclen/tw-nhi-service/actions/workflows/ci.yml/badge.svg)](https://github.com/magiclen/tw-nhi-service/actions/workflows/ci.yml)

透過 HTTP API 讀取中華民國健保卡。

Read Taiwan NHI cards via HTTP API.

## 用法

#### 執行環境

###### Windows / macOS

請先安裝好您讀卡機的驅動程式。

###### Linux

需有 `pcscd` (來自 [PCSClite project](https://pcsclite.apdu.fr/))。

基於 Debian 的 Linux 發行版可用以下指令安裝：

```bash
sudo apt install pcscd libpcsclite1
```

接著安裝好您讀卡機的驅動程式。

#### 開發環境

若要在 GNU/Linux 下編譯本專案，需要 `libpcsclite-dev` 套件。

基於 Debian 的 Linux 發行版可用以下指令安裝：

```bash
sudo apt install libpcsclite-dev
```

#### 命令列介面 (CLI)

```text
EXAMPLES:
tw-nhi-service                      # 啟動 HTTP 服務，監聽 127.0.0.1:58113
tw-nhi-service -i 0.0.0.0 -p 12345  # 啟動 HTTP 服務，監聽 0.0.0.0:12345

Usage: tw-nhi-service [OPTIONS]

Options:
  -i, --interface <INTERFACE>  要監聽的網路介面 IP [default: 127.0.0.1] [aliases: ip]
  -p, --port <PORT>            要監聽的連接埠 [default: 58113]
  -h, --help                   Print help
  -V, --version                Print version
```

#### HTTP API

啟動 HTTP 服務後，可以存取以下的端點：

* GET `/`：讀取所有讀卡機的健保卡中的基本資料。回應的 Content-Type 為 `application/json`。JSON 格式如下：
    ```json
    [
        {
            "reader_name": "讀卡機名稱",
            "card_no": "卡號",
            "full_name": "全名",
            "id_no": "身份證字號",
            "birth_date": "0000-00-00",
            "sex": "M：男；F：女",
            "issue_date": "0000-00-00",
        },
  
        ...
    ]
    ```
* GET `/version`：回傳此服務的版本，可用來檢驗此服務是否正常在監聽。回應的 Content-Type 為 `text/plain`。

## License

[MIT](LICENSE)