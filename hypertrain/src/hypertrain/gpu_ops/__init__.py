"""Paid GPU operations (Vast): journal-resumable launcher, deadline supervisor, budget gate.

Every provider write is refused unless the base URL is loopback (mock) or ``--live`` is passed.
"""
