from fastapi import Depends, FastAPI

from .dependencies import audit_request
from .routers import items

app = FastAPI(dependencies=[Depends(audit_request)])


@app.middleware("http")
async def timing_middleware(request, call_next):
    return await call_next(request)


app.include_router(items.router, prefix="/api", dependencies=[Depends(audit_request)])


@app.get("/health")
async def health():
    return {"ok": True}
