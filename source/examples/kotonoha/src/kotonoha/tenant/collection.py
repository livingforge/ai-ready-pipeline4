"""コレクションの作成と参照。

**機密区分の継承がここで効く。** テナントの区分を引き継ぎ、それより緩い
区分は指定できない（``classification.ensure_not_lowered``）。埋め込みモデルは
作成時に固定で、後から変えるには再インデックスが要る —— 次元とベクトル空間が
変わるので、混ぜると検索が壊れるためである。
"""

from __future__ import annotations

from kotonoha.common import ids
from kotonoha.common import logging as applog
from kotonoha.common.errors import AlreadyExists, InvalidInput, NotFound
from kotonoha.embed import registry
from kotonoha.tenant import classification as cls
from kotonoha.tenant.models import Collection

log = applog.get(__name__)


class CollectionService:
    """コレクションの一生を見る。"""

    def __init__(self, collection_repo, tenant_repo) -> None:
        self._collections = collection_repo
        self._tenants = tenant_repo

    def create(self, tenant_id: str, name: str, *,
               classification: str | None = None,
               embed_model: str | None = None) -> Collection:
        """作る。

        :param classification: 継承より**厳しい**区分にしたいときだけ指定する
        :param embed_model: 省略するとテナントの既定モデル
        :raises InvalidInput: 名前の形が不正／区分に使えないモデル
        :raises AlreadyExists: 同じ名前が既にある
        :raises ClassificationViolation: 継承より緩い区分を指定した
        """
        tenant = self._tenant(tenant_id)
        if not ids.valid_collection_name(name):
            raise InvalidInput(f"コレクション名の形が不正です: {name}", name=name)
        if self._collections.find_by_name(tenant_id, name) is not None:
            raise AlreadyExists(f"同じ名前のコレクションがあります: {name}", name=name)

        level = cls.ensure_not_lowered(tenant.classification, classification)
        model = registry.resolve(_requested_model(tenant, embed_model, level), level)

        collection_id = ids.new_id()
        collection = Collection(
            collection_id=collection_id,
            tenant_id=tenant_id,
            collection_name=name,
            classification=level,
            embed_model=model.name,
            embed_dim=model.dim,
            index_alias=ids.index_alias(collection_id),
        )
        self._collections.save(collection)
        log.info("コレクションを作成しました tenant=%s name=%s class=%s model=%s",
                 tenant_id, name, level, model.name)
        return collection

    def get(self, collection_id: str, *, tenant_id: str | None = None) -> Collection:
        """引く。``tenant_id`` を渡すと他テナントのものを弾く。

        :raises NotFound: 無い／他テナントのもの
        """
        collection = self._collections.find(collection_id)
        if collection is None or collection.status == "D":
            raise NotFound(f"コレクションがありません: {collection_id}",
                           collection_id=collection_id)
        if tenant_id is not None and collection.tenant_id != tenant_id:
            # **「他テナントのもの」とは言わない。**存在を探れてしまう。
            raise NotFound(f"コレクションがありません: {collection_id}",
                           collection_id=collection_id)
        return collection

    def list_for(self, tenant_id: str) -> list[Collection]:
        """テナントのコレクション一覧。削除済みは出さない。"""
        return [c for c in self._collections.list_by_tenant(tenant_id) if c.status != "D"]

    def mark_rebuilding(self, collection_id: str) -> Collection:
        """再構築中にする。取り込みは受けなくなるが検索は続く。"""
        collection = self.get(collection_id)
        collection.status = "R"
        self._collections.save(collection)
        return collection

    def mark_active(self, collection_id: str) -> Collection:
        """再構築を終えて通常へ戻す。"""
        collection = self.get(collection_id)
        collection.status = "A"
        self._collections.save(collection)
        return collection

    def _tenant(self, tenant_id: str):
        tenant = self._tenants.find(tenant_id)
        if tenant is None or not tenant.active:
            raise NotFound(f"テナントがありません: {tenant_id}", tenant_id=tenant_id)
        return tenant


def _requested_model(tenant, explicit: str | None, level: str) -> str | None:
    """``registry.resolve`` へ渡すモデル名を決める。

    **明示の指定とテナント既定を区別する。**

    - 明示に指定されたものは**そのまま渡す** —— 区分に許されなければ
      ``ClassificationViolation`` で弾かれるのが正しい。「voyage-law-2 を
      指定したのに黙って別のモデルで埋められた」を起こさないため。
    - テナント既定は**あくまで既定**なので、コレクションの区分で使えない
      ときは ``None`` を返して区分に応じた既定へ倒す —— 一般のテナントが
      極秘のコレクションを作るときに、既定が外部モデルだからという理由で
      作れないのはおかしい。

    この区別はどの設計文書にも書かれていない。
    """
    if explicit:
        return explicit
    if tenant.embed_model and cls.allows_external(level):
        return tenant.embed_model
    if tenant.embed_model and not registry.get(tenant.embed_model).external:
        return tenant.embed_model
    return None
