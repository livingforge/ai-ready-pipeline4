# ARP プロジェクト導入用配布物

この配布物の `.arp/bootstrap/` を利用先の同じ場所へ配置します。既存の `src/`・`tests/`・README・Python設定にはファイルを展開しません。

```powershell
python .arp/bootstrap/install.py --root .
```

コピーせず、この配布物から対象を指定することもできます。

```powershell
python C:/arp4-publish/.arp/bootstrap/install.py --root C:/my-project
```

別のOS・Pythonでは、明示的にオンライン導入を選択できます。

```shell
python .arp/bootstrap/install.py --root . --mode online
```

Python 3.11以上が必要です。オフライン対応環境はmanifest.jsonのenvironmentを参照してください。
オンラインでは共通ARP wheelを使い、実行環境に適合する依存wheelを取得します。
社内ミラーは `--index-url <URL>`、依存の再現は `--lock <記録した.lock>` を指定します。
ロック未指定時は依存を解決し、選択結果を `.arp/installations/<環境>/<hash>.lock` に記録します。
CIの検証対象はWindows x64とUbuntu x64、CPython 3.11〜3.14です。
CI成功の環境別配布物はリポジトリのActions成果物から取得してください。Alpine・ARM・macOSは未検証です。
依存wheelを同梱し、ハッシュ検証付きでオフライン導入します。
`.arp/runtime/` に専用環境、`.arp/config.yml` と `knowledge/` に文書管理を作成します。
既存のARP配置設定は維持します。未管理のknowledge/やruntime/がある場合は停止します。
パース・文書管理・XMLセル更新は共通機能です。行・図形・数式の高度な書き戻しにはWindows＋Microsoft Excelが必要です。

実行例: `.arp/runtime/Scripts/python.exe -m arp4 documents --help`
Linux/macOSでは `.arp/runtime/bin/python` を使用します。
文書管理の仕様は [documents.md](documents.md) を参照してください。
