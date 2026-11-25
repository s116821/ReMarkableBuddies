// Keep final-target compiler flags coupled to build-script admission. This is
// included by both product crate roots; it does not enable the diagnostic.
#[cfg(all(
    buddy_development_native_pre_enter,
    not(buddy_development_native_pre_enter_admitted)
))]
compile_error!("Development pre-Enter build admission refused");

#[cfg(all(
    buddy_development_native_pre_enter,
    not(feature = "development-resident-bootstrap")
))]
compile_error!("Development pre-Enter requires development-resident-bootstrap");

#[cfg(all(
    buddy_development_native_composer,
    not(buddy_development_native_composer_admitted)
))]
compile_error!("Development composer build admission refused");

#[cfg(all(
    buddy_development_native_composer,
    not(feature = "development-resident-bootstrap")
))]
compile_error!("Development composer requires development-resident-bootstrap");
