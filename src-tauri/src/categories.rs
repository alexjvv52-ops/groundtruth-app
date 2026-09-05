//! Canonical cost categories — structured data, dual Schedule F/C mapping.
//!
//! Authority: BOOKS-BOUNDARY §2 / §4; ROADMAP Track 3.
//! The operator sees plain-language names only. Line numbers never appear in UI.
//! No category carries an amount, rate, default price, or per-unit cost.

use serde::{Deserialize, Serialize};

/// One controlled category. Field set is what `categories.json` will emit later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostCategory {
    /// Stable id stored on the cost event (`canonical_category`).
    pub id: &'static str,
    /// Plain-language name the operator picks.
    pub name: &'static str,
    /// Schedule F line (or "32 other").
    pub schedule_f_line: &'static str,
    /// Schedule C line (or "27b other").
    pub schedule_c_line: &'static str,
    /// Mandatory free-text when either mapping is an "other" line.
    pub descriptor_required: bool,
}

/// The closed list. Order is display order.
pub const COST_CATEGORIES: &[CostCategory] = &[
    CostCategory {
        id: "seed",
        name: "Seed",
        schedule_f_line: "26",
        schedule_c_line: "22",
        descriptor_required: false,
    },
    CostCategory {
        id: "growing_medium",
        name: "Growing medium",
        schedule_f_line: "28",
        schedule_c_line: "22",
        descriptor_required: false,
    },
    CostCategory {
        id: "trays_domes_racks",
        name: "Trays, domes and racks",
        schedule_f_line: "28",
        schedule_c_line: "22",
        descriptor_required: false,
    },
    CostCategory {
        id: "packaging_labels",
        name: "Packaging and labels",
        schedule_f_line: "28",
        schedule_c_line: "22",
        descriptor_required: false,
    },
    CostCategory {
        id: "cleaning_sanitizing",
        name: "Cleaning and sanitizing",
        schedule_f_line: "11",
        schedule_c_line: "22",
        descriptor_required: false,
    },
    CostCategory {
        id: "nutrients_amendments",
        name: "Nutrients and amendments",
        schedule_f_line: "17",
        schedule_c_line: "22",
        descriptor_required: false,
    },
    CostCategory {
        id: "electricity_water_heat",
        name: "Electricity, water and heat",
        schedule_f_line: "30",
        schedule_c_line: "25",
        descriptor_required: false,
    },
    CostCategory {
        id: "grow_space_rent",
        name: "Grow space rent",
        schedule_f_line: "24b",
        schedule_c_line: "20b",
        descriptor_required: false,
    },
    CostCategory {
        id: "repairs_maintenance",
        name: "Repairs and maintenance",
        schedule_f_line: "25",
        schedule_c_line: "21",
        descriptor_required: false,
    },
    CostCategory {
        id: "inbound_freight",
        name: "Inbound freight and shipping",
        schedule_f_line: "18",
        schedule_c_line: "22",
        descriptor_required: false,
    },
    CostCategory {
        id: "delivery_fuel",
        name: "Delivery fuel",
        schedule_f_line: "19",
        schedule_c_line: "9",
        descriptor_required: false,
    },
    CostCategory {
        id: "insurance",
        name: "Insurance",
        schedule_f_line: "20",
        schedule_c_line: "15",
        descriptor_required: false,
    },
    CostCategory {
        id: "tools_small_equipment",
        name: "Tools and small equipment",
        schedule_f_line: "28",
        schedule_c_line: "22",
        descriptor_required: false,
    },
    CostCategory {
        id: "market_stall_booth",
        name: "Market stall or booth fee",
        schedule_f_line: "32 other",
        schedule_c_line: "27b other",
        descriptor_required: true,
    },
    CostCategory {
        id: "advertising_printing",
        name: "Advertising and printing",
        schedule_f_line: "32 other",
        schedule_c_line: "8",
        descriptor_required: true,
    },
    CostCategory {
        id: "website_pos_fees",
        name: "Website, POS and payment fees",
        schedule_f_line: "32 other",
        schedule_c_line: "27b other",
        descriptor_required: true,
    },
    CostCategory {
        id: "professional_fees",
        name: "Professional fees",
        schedule_f_line: "32 other",
        schedule_c_line: "17",
        descriptor_required: true,
    },
    CostCategory {
        id: "office_admin_supplies",
        name: "Office and admin supplies",
        schedule_f_line: "32 other",
        schedule_c_line: "18",
        descriptor_required: true,
    },
];

pub fn find_category(id: &str) -> Option<&'static CostCategory> {
    COST_CATEGORIES.iter().find(|c| c.id == id)
}

/// True when either tax mapping is an "other" line (descriptor mandatory).
pub fn line_is_other(line: &str) -> bool {
    line.to_ascii_lowercase().contains("other")
}

/// Operator-facing view — no Schedule F/C line numbers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostCategoryView {
    pub id: String,
    pub name: String,
    pub descriptor_required: bool,
}

pub fn list_categories() -> Vec<CostCategoryView> {
    COST_CATEGORIES
        .iter()
        .map(|c| CostCategoryView {
            id: c.id.to_string(),
            name: c.name.to_string(),
            descriptor_required: c.descriptor_required,
        })
        .collect()
}

/// Full serialisable record for export (`categories.json`) — not shown in UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostCategoryExport {
    pub id: String,
    pub name: String,
    pub schedule_f_line: String,
    pub schedule_c_line: String,
    pub descriptor_required: bool,
}

pub fn export_categories() -> Vec<CostCategoryExport> {
    COST_CATEGORIES
        .iter()
        .map(|c| CostCategoryExport {
            id: c.id.to_string(),
            name: c.name.to_string(),
            schedule_f_line: c.schedule_f_line.to_string(),
            schedule_c_line: c.schedule_c_line.to_string(),
            descriptor_required: c.descriptor_required,
        })
        .collect()
}

/// One controlled income category. Same shape as cost categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IncomeCategory {
    /// Stable id stored on the income event (`canonical_category`).
    pub id: &'static str,
    /// Plain-language name the operator picks.
    pub name: &'static str,
    /// Schedule F line (or "8 other").
    pub schedule_f_line: &'static str,
    /// Schedule C line (or "6 other").
    pub schedule_c_line: &'static str,
    /// Mandatory free-text when either mapping is an "other" line.
    pub descriptor_required: bool,
}

/// The closed list. Order is display order. Six categories — invent no seventh.
pub const INCOME_CATEGORIES: &[IncomeCategory] = &[
    IncomeCategory {
        id: "produce_you_grew",
        name: "Produce you grew",
        schedule_f_line: "2",
        schedule_c_line: "1",
        descriptor_required: false,
    },
    IncomeCategory {
        id: "resold_goods",
        name: "Goods you bought to resell",
        schedule_f_line: "1b",
        schedule_c_line: "1",
        descriptor_required: false,
    },
    IncomeCategory {
        id: "custom_work",
        name: "Custom work or hire",
        schedule_f_line: "7",
        schedule_c_line: "1",
        descriptor_required: false,
    },
    IncomeCategory {
        id: "program_payment",
        name: "Agricultural program payment",
        schedule_f_line: "4b",
        schedule_c_line: "6 other",
        descriptor_required: true,
    },
    IncomeCategory {
        id: "crop_insurance",
        name: "Crop insurance or disaster payment",
        schedule_f_line: "6a",
        schedule_c_line: "6 other",
        descriptor_required: true,
    },
    IncomeCategory {
        id: "other_farm_income",
        name: "Other farm income",
        schedule_f_line: "8 other",
        schedule_c_line: "6 other",
        descriptor_required: true,
    },
];

pub fn find_income_category(id: &str) -> Option<&'static IncomeCategory> {
    INCOME_CATEGORIES.iter().find(|c| c.id == id)
}

/// Operator-facing view — no Schedule F/C line numbers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomeCategoryView {
    pub id: String,
    pub name: String,
    pub descriptor_required: bool,
}

pub fn list_income_categories() -> Vec<IncomeCategoryView> {
    INCOME_CATEGORIES
        .iter()
        .map(|c| IncomeCategoryView {
            id: c.id.to_string(),
            name: c.name.to_string(),
            descriptor_required: c.descriptor_required,
        })
        .collect()
}
