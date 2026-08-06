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

    def test_earnings_rows_use_current_baostock_field_names(self):
        source = source_module.BaostockSource()
        forecast = source._forecast_row(
            {
                "profitForcastExpPubDate": "2026-01-15",
                "profitForcastExpStatDate": "2025-12-31",
                "profitForcastChgPctUp": "40",
                "profitForcastChgPctDwn": "20",
            },
            "600519.SH",
        )
        express = source._express_row(
            {
                "performanceExpPubDate": "2026-04-01",
                "performanceExpStatDate": "2026-03-31",
            },
            "600519.SH",
        )

        self.assertEqual(forecast["ann_date"].isoformat(), "2026-01-15")
        self.assertEqual(forecast["report_period"].isoformat(), "2025-12-31")
        self.assertEqual(forecast["net_profit_yoy"], 30.0)
        self.assertEqual(express["ann_date"].isoformat(), "2026-04-01")
        self.assertEqual(express["report_period"].isoformat(), "2026-03-31")

    def test_financial_values_keep_current_sdk_units(self):
        mapped = source_module.BaostockSource()._map_profit(
            {
                "MBRevenue": "170611838052.02",
                "netProfit": "89334728025.90",
                "epsTTM": "68.64",
            }
        )

        self.assertEqual(mapped["revenue"], 170611838052.02)
        self.assertEqual(mapped["net_profit"], 89334728025.90)


if __name__ == "__main__":
    unittest.main()
