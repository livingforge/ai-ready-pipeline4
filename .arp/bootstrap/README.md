# ARP プロジェクト導入用配布物

この配布物の `.arp/bootstrap/` を利用先の同じ場所へ配置します。既存の `src/`・`tests/`・README・Python設定にはファイルを展開しません。

```powershell
python .arp/bootstrap/install.py --root .
```

コピーせず、この配布物から対象を指定することもできます。

```powershell
python C:/arp4-publish/.arp/bootstrap/install.py --root C:/my-project
```

Python 3.11以上が必要です。対応Pythonマイナーバージョン・OS・CPUはmanifest.jsonのenvironmentを参照してください。他環境にはその環境で作成した配布物を使用します。
依存wheelを同梱し、ハッシュ検証付きでオフライン導入します。
`.arp/runtime/` に専用環境、`.arp/config.yml` と `knowledge/` に文書管理を作成します。
既存のARP配置設定は維持します。未管理のknowledge/やruntime/がある場合は停止します。
Windowsの実Excelを使う書き戻しにはMicrosoft Excelのインストールも必要です。

実行例: `.arp/runtime/Scripts/python.exe -m arp4 documents --help`
Linux/macOSでは `.arp/runtime/bin/python` を使用します。
文書管理の仕様は [documents.md](documents.md) を参照してください。
