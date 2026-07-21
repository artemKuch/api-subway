from fastapi import Depends


def session_scope():
    return "session"


def current_user(session=Depends(session_scope)):
    return {"id": "1"}


def audit_request(user=Depends(current_user)):
    return user
