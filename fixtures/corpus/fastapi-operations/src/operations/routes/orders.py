from fastapi import APIRouter, Depends

from operations.dependencies import require_tenant
from operations.services.orders import create_order, list_orders, load_order

router = APIRouter(prefix="/orders", dependencies=[Depends(require_tenant)])


@router.get("")
async def get_orders():
    return await list_orders()


@router.post("", status_code=201)
async def post_order(payload: dict):
    return await create_order(payload)


@router.get("/{order_id}")
async def get_order(order_id: str):
    return await load_order(order_id)
