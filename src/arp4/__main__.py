"""``python -m arp4`` で CLI を起動する（venv 未整備でも動かせるように）。"""

from arp4.cli import main

if __name__ == "__main__":
    raise SystemExit(main())
