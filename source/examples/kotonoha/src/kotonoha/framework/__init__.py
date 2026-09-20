"""FastAPI と Pydantic の代わりに自前で用意した最小の足場。

**設計文書に対応する成果物ではない。** 本物の依存を入れると pip とネットワークが
必要になり、「配るだけで動かせる」が崩れるので置いてある。``@get`` ``@post`` や
``Schema`` の書き方は本番と同じ形になるので、資材としての見た目は保たれる。

置き換えるときの対応:

===================  ==========================================
ここにあるもの        本番で使うもの
===================  ==========================================
``Router`` / ``App``  ``fastapi.APIRouter`` / ``FastAPI``
``Schema``            ``pydantic.BaseModel``
``HttpError``         ``fastapi.HTTPException``
``serve``             ``uvicorn.run``
===================  ==========================================
"""

from kotonoha.framework.routing import App, Request, Response, Router
from kotonoha.framework.schema import Field, Schema, ValidationError
from kotonoha.framework.errors import HttpError

__all__ = [
    "App", "Request", "Response", "Router",
    "Field", "Schema", "ValidationError", "HttpError",
]
