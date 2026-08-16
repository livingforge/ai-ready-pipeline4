"""リクエスト・レスポンスの型。``pydantic.BaseModel`` に相当する。

型注釈と ``Field`` から検証する。**必要な分だけ**で、pydantic の機能を
真似ようとはしていない（型・必須・最小最大・列挙）。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, get_args, get_origin, get_type_hints


class ValidationError(Exception):
    """入力検証に落ちた。``errors`` に項目ごとの理由が入る。"""

    def __init__(self, errors: list[tuple[str, str]]) -> None:
        super().__init__("; ".join(f"{f}: {m}" for f, m in errors))
        self.errors = errors


@dataclass
class Field:
    """項目の制約。既定値が必要なら ``default`` を入れる。"""

    default: Any = ...
    min_length: int | None = None
    max_length: int | None = None
    ge: float | None = None
    le: float | None = None
    choices: tuple[Any, ...] | None = None
    description: str = ""

    @property
    def required(self) -> bool:
        return self.default is ...


class Schema:
    """入力の型。継承して型注釈と ``Field`` を書く。

    ::

        class SearchRequest(Schema):
            query: str = Field(min_length=1, max_length=2000)
            top_k: int = Field(default=10, ge=1, le=100)
    """

    def __init__(self, **values: Any) -> None:
        errors: list[tuple[str, str]] = []
        for name, kind in self._fields().items():
            spec = self._spec(name)
            if name not in values or values[name] is None:
                if spec.required:
                    errors.append((name, "必須です"))
                else:
                    setattr(self, name, spec.default)
                continue
            value = values[name]
            problem = _check(value, kind, spec)
            if problem:
                errors.append((name, problem))
            else:
                setattr(self, name, value)
        unknown = set(values) - set(self._fields())
        for name in sorted(unknown):
            errors.append((name, "知らない項目です"))
        if errors:
            raise ValidationError(errors)

    @classmethod
    def _fields(cls) -> dict[str, Any]:
        """項目名 → 型。**文字列の注釈を解決してから返す。**

        ``from __future__ import annotations`` を書いたモジュールでは
        ``__annotations__`` が文字列になる。そのままだと ``list[str]`` が
        ただの文字列として扱われ、**配列の検証が丸ごと効かなくなる** ——
        件数の上限も要素の型も見ないまま通ってしまう。
        """
        try:
            return dict(get_type_hints(cls))
        except Exception:            # 解決できない注釈があっても止めない
            fields: dict[str, Any] = {}
            for klass in reversed(cls.__mro__):
                fields.update(getattr(klass, "__annotations__", {}))
            return fields

    @classmethod
    def _spec(cls, name: str) -> Field:
        spec = getattr(cls, name, ...)
        return spec if isinstance(spec, Field) else Field(default=spec)

    def to_dict(self) -> dict:
        return {n: getattr(self, n, None) for n in self._fields()}

    def __repr__(self) -> str:
        inner = ", ".join(f"{k}={v!r}" for k, v in self.to_dict().items())
        return f"{type(self).__name__}({inner})"


def _check(value: Any, kind: Any, spec: Field) -> str | None:
    """1 項目を検証し、駄目なら理由を返す。"""
    origin = get_origin(kind)
    if origin is list:
        if not isinstance(value, list):
            return "配列で指定してください"
        (item_kind,) = get_args(kind) or (Any,)
        for item in value:
            problem = _check(item, item_kind, Field(default=None))
            if problem:
                return f"要素が不正です（{problem}）"
        if spec.max_length is not None and len(value) > spec.max_length:
            return f"{spec.max_length} 件までです"
        if spec.min_length is not None and len(value) < spec.min_length:
            return f"{spec.min_length} 件以上必要です"
        return None
    if kind is Any or kind is None:
        return None
    if isinstance(kind, type) and not isinstance(value, kind):
        if not (kind is float and isinstance(value, int)):
            return f"{kind.__name__} で指定してください"
    if isinstance(value, str):
        if spec.min_length is not None and len(value) < spec.min_length:
            return f"{spec.min_length} 文字以上必要です"
        if spec.max_length is not None and len(value) > spec.max_length:
            return f"{spec.max_length} 文字までです"
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if spec.ge is not None and value < spec.ge:
            return f"{spec.ge} 以上で指定してください"
        if spec.le is not None and value > spec.le:
            return f"{spec.le} 以下で指定してください"
    if spec.choices is not None and value not in spec.choices:
        return f"{'/'.join(map(str, spec.choices))} のいずれかです"
    return None
