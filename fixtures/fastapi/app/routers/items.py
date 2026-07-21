from typing import Annotated, Literal
from uuid import UUID

from fastapi import APIRouter, Depends
from pydantic import BaseModel
from sqlalchemy import select

from ..dependencies import current_user
from ..services.catalog import sync_catalog

router = APIRouter(prefix="/items", dependencies=[Depends(current_user)])


class ItemInput(BaseModel):
    name: str
    role: Literal["admin", "member"] = "member"


class ItemOutput(BaseModel):
    id: UUID
    name: str
    role: Literal["admin", "member"]


@router.get("/{item_id}")
async def get_item(item_id: str, user: Annotated[dict, Depends(current_user)]):
    select(item_id)
    return user


@router.api_route(
    "/sync", methods=["POST", "PUT"], response_model=ItemOutput, status_code=201
)
async def sync_items(payload: ItemInput) -> ItemOutput:
    return await sync_catalog(payload)
