import types
import unittest
from unittest.mock import patch

from myfin_py.sources import SourceError
from myfin_py.sources import baostock_source as source_module


class BaostockUnavailableTest(unittest.TestCase):
    def setUp(self):
        source_module._blacklisted_until = 0.0

    def test_blacklist_login_returns_empty_daily_frame(self):
        response = types.SimpleNamespace(error_code="10001011", error_msg="黑名单用户")
        with patch.object(source_module.bs, "login", return_value=response):
            frame = source_module.BaostockSource().fetch_daily(
                "600519.SH", "2026-01-01", "2026-01-05"
            )

        self.assertTrue(frame.empty)
        self.assertEqual(list(frame.columns), source_module.empty_frame("daily").columns.tolist())

    def test_invalid_credentials_still_raise(self):
        response = types.SimpleNamespace(error_code="10001002", error_msg="用户名或密码错误")
        with patch.object(source_module.bs, "login", return_value=response):
            with self.assertRaises(SourceError) as caught:
                source_module.BaostockSource().fetch_daily(
                    "600519.SH", "2026-01-01", "2026-01-05"
                )

        self.assertEqual(caught.exception.operation, "login")


if __name__ == "__main__":
    unittest.main()
