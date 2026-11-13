//! All registered actions/menus must explicitly declare their navigation meaning.
//!
//! This is an exhaustive semantic inventory, not a name-prefix allowlist. Adding
//! an enum variant fails compilation here until its behavior has been reviewed.
//! The registry parity tests in keymap.rs separately catch unregistered variants.
#![cfg(windows)]

use super::*;
use crate::keymap::{KeyAction, MenuCommandId};
use crate::ui_main::{AddressBarNav, LocationJumpTarget};
use egui_kittest::kittest::Queryable;
use std::cell::RefCell;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    Location,
    Favorite,
    SavedGroup,
    FolderDialog,
    PhysicalParent,
    BookManager,
    CollectionManager,
    // Open/parent/previous/next within the accepted result or reader session.
    ResultNavigation,
    // History refuses an active search; it does not create a pseudo-child.
    History,
    Refresh,
    SearchSurface,
    External,
    NoLocationChange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuEntry {
    FolderDialog,
    ReadingHistory,
    BooksRoot,
    SavedBook,
    SavedCollection,
    BookManager,
    CollectionManager,
    SearchSurface,
    Refresh,
    LogicalView,
    External,
    NoLocationChange,
}

fn key_entry(action: KeyAction) -> Entry {
    use KeyAction::*;
    match action {
        GridOpenDriveC
        | GridOpenDriveD
        | GridOpenDriveE
        | GridOpenDriveF
        | GridOpenDriveG
        | GridOpenDriveH
        | GridOpenDriveI
        | GridOpenDriveJ
        | GridOpenDriveK
        | GridOpenDriveL
        | GridOpenDriveM
        | GridOpenDriveN
        | GridOpenDriveO
        | GridOpenDriveP
        | GridOpenDriveQ
        | GridOpenDriveR
        | GridOpenDriveS
        | GridOpenDriveT
        | GridOpenDriveU
        | GridOpenDriveV
        | GridOpenDriveW
        | GridOpenDriveX
        | GridOpenDriveY
        | GridOpenDriveZ
        | GridOpenCurrentDriveRoot
        | GridSwitchDriveC
        | GridSwitchDriveD
        | GridSwitchDriveE
        | GridSwitchDriveF
        | GridSwitchDriveG
        | GridSwitchDriveH
        | GridSwitchDriveI
        | GridSwitchDriveJ
        | GridSwitchDriveK
        | GridSwitchDriveL
        | GridSwitchDriveM
        | GridSwitchDriveN
        | GridSwitchDriveO
        | GridSwitchDriveP
        | GridSwitchDriveQ
        | GridSwitchDriveR
        | GridSwitchDriveS
        | GridSwitchDriveT
        | GridSwitchDriveU
        | GridSwitchDriveV
        | GridSwitchDriveW
        | GridSwitchDriveX
        | GridSwitchDriveY
        | GridSwitchDriveZ
        | GridOpenLocationDriveList
        | GridOpenLocationReadingHistory
        | GridOpenLocationRating1
        | GridOpenLocationRating2
        | GridOpenLocationRating3
        | GridOpenLocationRating4
        | GridOpenLocationRating5
        | GridOpenLocationBooksRoot
        | GridOpenLocationDesktop
        | GridOpenLocationPictures
        | GridOpenLocationDownloads => Entry::Location,
        GridFavoritePrev | GridFavoriteNext | GridOpenFavorite1 | GridOpenFavorite2
        | GridOpenFavorite3 | GridOpenFavorite4 | GridOpenFavorite5 | GridOpenFavorite6
        | GridOpenFavorite7 | GridOpenFavorite8 | GridOpenFavorite9 | GridOpenFavorite10
        | GridOpenFavorite11 | GridOpenFavorite12 | GridOpenFavorite13 | GridOpenFavorite14
        | GridOpenFavorite15 | GridOpenFavorite16 | GridOpenFavorite17 | GridOpenFavorite18
        | GridOpenFavorite19 | GridOpenFavorite20 => Entry::Favorite,
        GridBookPrev
        | GridBookNext
        | GridOpenActiveBook
        | GridCollectionPrev
        | GridCollectionNext
        | GridOpenCollectionTarget
        | GridOpenBook1
        | GridOpenBook2
        | GridOpenBook3
        | GridOpenBook4
        | GridOpenBook5
        | GridOpenBook6
        | GridOpenBook7
        | GridOpenBook8
        | GridOpenBook9
        | GridOpenBook10
        | GridOpenBook11
        | GridOpenBook12
        | GridOpenBook13
        | GridOpenBook14
        | GridOpenBook15
        | GridOpenBook16
        | GridOpenBook17
        | GridOpenBook18
        | GridOpenBook19
        | GridOpenBook20
        | GridOpenCollection1
        | GridOpenCollection2
        | GridOpenCollection3
        | GridOpenCollection4
        | GridOpenCollection5
        | GridOpenCollection6
        | GridOpenCollection7
        | GridOpenCollection8
        | GridOpenCollection9
        | GridOpenCollection10
        | GridOpenCollection11
        | GridOpenCollection12
        | GridOpenCollection13
        | GridOpenCollection14
        | GridOpenCollection15
        | GridOpenCollection16
        | GridOpenCollection17
        | GridOpenCollection18
        | GridOpenCollection19
        | GridOpenCollection20 => Entry::SavedGroup,
        GlobalOpenFolder => Entry::FolderDialog,
        GridPhysicalParentFolder => Entry::PhysicalParent,
        GridManageBooks => Entry::BookManager,
        GridManageCollections => Entry::CollectionManager,
        GridOpenSelected
        | GridOpenSelectedAsPage
        | GridOpenSelectedAsList
        | GridParentFolder
        | GridTreeFolderPrev
        | GridTreeFolderNext
        | GridSiblingFolderPrev
        | GridSiblingFolderNext
        | FsClose
        | FsBackToList
        | VideoCloseFullscreen
        | FsCtrlNavPrev
        | FsCtrlNavNext
        | FsListOrderPrev
        | FsListOrderNext
        | FsSiblingPrev
        | FsSiblingNext
        | VideoNextFile
        | VideoPrevFile => Entry::ResultNavigation,
        GridHistoryBack | GridHistoryForward => Entry::History,
        GridReload => Entry::Refresh,
        GlobalLocalSearch
        | GlobalFavSearch
        | GlobalMetadataSearch
        | GlobalEverythingSearch
        | GridTagView
        // §1.690: put the selected item's name into a search box without
        // running it; the location changes only after the user presses Enter.
        | GridSearchNameEverything
        | GridSearchNameMetadata
        | GridSearchNameLocal => Entry::SearchSurface,
        GridOpenExternalPlayer
        | VideoExternalPlayer
        | ExternalToolPicker
        | ExternalToolForContainer
        | ExternalTool1
        | ExternalTool2
        | ExternalTool3
        | ExternalTool4
        | ExternalTool5
        | ExternalTool6
        | ExternalTool7
        | ExternalTool8
        | ExternalTool9
        | ExternalTool10 => Entry::External,
        ToggleDetachedViewerMode
        | ToggleAlwaysOnTop
        | ClearVideoMediaMemory
        | ClearBookResumePositions
        | UndoLastAction
        | HelpShowContextShortcuts
        | GridManageFavorites
        | GridClearRecentFolders
        | GridClearSearchInputHistory
        | GridClearReadingHistory
        | GridClearQuickFolderSlots
        | GridTogglePinnedTag1
        | GridTogglePinnedTag2
        | GridTogglePinnedTag3
        | GridTogglePinnedTag4
        | GridTogglePinnedTag5
        | GridTogglePinnedTag6
        | GridTogglePinnedTag7
        | GridTogglePinnedTag8
        | GridTogglePinnedTag9
        | GridTogglePinnedTag10
        | GridTogglePinnedTag11
        | GridTogglePinnedTag12
        | GridTogglePinnedTag13
        | GridTogglePinnedTag14
        | GridTogglePinnedTag15
        | GridTogglePinnedTag16
        | GridTogglePinnedTag17
        | GridTogglePinnedTag18
        | GridTogglePinnedTag19
        | GridTogglePinnedTag20
        | GridSelectAll
        | GridDeselect
        | GridCutFiles
        | GridCopyFiles
        | GridCopyPath
        | GridCopyFileName
        | GridCopyImage
        | GridCopyEditedImage
        | GridOrganizeFiles
        | GridToggleCheck
        | GridDelete
        | GridRename
        | GridMoveFirst
        | GridMoveLast
        | GridPagePrev
        | GridPageNext
        | GridToggleMaximize
        | GridToggleFolderTreePane
        | GridToggleStackMode
        | GridConvertEpubToPdf
        | GridTagApply
        | GridRotateCw
        | GridRotateCcw
        | GridPin
        | GridComparePin
        | GridAddToActiveBook
        | GridAddToCollectionTarget
        | GridExportSelection
        | GridColumnCount1
        | GridColumnCount2
        | GridColumnCount3
        | GridColumnCount4
        | GridColumnCount5
        | GridColumnCount6
        | GridColumnCount7
        | GridColumnCount8
        | GridColumnCount9
        | GridColumnCount10
        | GridColumnCount11
        | GridColumnCount12
        | GridColumnCount13
        | GridColumnCount14
        | GridColumnCount15
        | GridColumnCount16
        | GridColumnCount17
        | GridColumnCount18
        | GridColumnCount19
        | GridColumnCount20
        | GridToggleDetailsView
        | GridOpenPreferences
        | GridOpenOperationCustomize
        | GridAdjustSlot1
        | GridAdjustSlot2
        | GridAdjustSlot3
        | GridAdjustSlot4
        | GridAdjustSlot5
        | GridAdjustSlot6
        | GridAdjustSlot7
        | GridAdjustSlot8
        | GridAdjustSlot9
        | GridAdjustSlot10
        | GridClearAdjust
        | GridApplyErase1
        | GridApplyErase2
        | GridApplyConceal1
        | GridApplyConceal2
        | GridDeleteEraseMask
        | GridDeleteConcealMask
        | FsToggleMetadata
        | FsPageOverviewToggle
        | FsPageOverviewConfirm
        | FsPageOverviewPagePrev
        | FsPageOverviewPageNext
        | FsPageOverviewFirst
        | FsPageOverviewLast
        | FsPageOverviewColumnsLess
        | FsPageOverviewColumnsMore
        | FsToggleWindowMode
        | FsJumpFirst
        | FsJumpLast
        | FsPagePrev
        | FsPageNext
        | FsFixedJumpPrev
        | FsFixedJumpNext
        | FsFixedJumpPrevNoRtl
        | FsFixedJumpNextNoRtl
        | FsStackJumpPrev
        | FsStackJumpNext
        | RatingItem1
        | RatingItem2
        | RatingItem3
        | RatingItem4
        | RatingItem5
        | RatingItemClear
        | RatingItemStepUp
        | RatingItemStepDown
        | RatingContainer1
        | RatingContainer2
        | RatingContainer3
        | RatingContainer4
        | RatingContainer5
        | RatingContainerClear
        | RatingContainerStepUp
        | RatingContainerStepDown
        | RatingBookPreferred1
        | RatingBookPreferred2
        | RatingBookPreferred3
        | RatingBookPreferred4
        | RatingBookPreferred5
        | RatingBookPreferredClear
        | RatingBookPreferredStepUp
        | RatingBookPreferredStepDown
        | FsContinuousScrollForward
        | FsContinuousScrollBack
        | FsSpreadShiftLeft
        | FsSpreadShiftRight
        | FsSpreadShiftPrev
        | FsSpreadShiftNext
        | FsTogglePageAfterCoverAlone
        | FsToggleLastPageAlone
        | FsSlideshow
        | FsSeekStripToggle
        | FsSpaceCheck
        | FsCapture
        | FsAddToActiveBook
        | FsAddToCollectionTarget
        | FsExport
        | FsCopyEditedImage
        | FsCopyFiles
        | FsCopyPath
        | FsCopyFileName
        | FsCopyImage
        | FsCompareToggle
        | FsCompareCycle
        | FsCompareWipe
        | FsCompareDiff
        | FsRotateCw
        | FsRotateCcw
        | FsImageAnalysis
        | FsZoomMode
        | FsScrubZoomHold
        | FsPanorama
        | FsPanoramaProjection
        | FsDepthParallax
        | FsNavigatorToggle
        | FsPageTurnAnimationToggle
        | FsNavigatorHold
        | FsPixelGrid
        | FsLoupeLockToggle
        | FsLoupeHold
        | FsOriginalPreviewHold
        | FsEraseMode
        | FsLocalAdjustMode
        | FsConcealMode
        | FsTextMode
        | FsSnsSplitMode
        | FsBookBookmark
        | FsBgCycle
        | FsPin
        | FsSpreadSingle
        | FsViewSpread
        | FsViewLandscapeSplit
        | FsSpreadLtr
        | FsSpreadLtrCover
        | FsSpreadRtl
        | FsSpreadRtlCover
        | FsSpreadSplitLtr
        | FsSpreadSplitRtl
        | FsReadingFlowCycle
        | FsReadingDirectionToggle
        | FsFitModeCycle
        | FsZoomIn
        | FsZoomOut
        | FsAiModelNext
        | FsAiModelPrev
        | FsAiModelReset
        | FsAiModelAuto
        | FsAiModelRealEsrganX4Plus
        | FsAiModelRealEsrganAnime6B
        | FsAiModelRealCugan4x
        | FsAiModelNmkdSiax4x
        | FsAiModelRealEsrGeneralV3
        | FsDenoiseCycle
        | FsPostFilterNext
        | FsPostFilterPrev
        | FsPostFilterReset
        | FsPostFilterNearest
        | FsPostFilterUpscaleSharp
        | FsPostFilterUpscaleAnime
        | FsPostFilterUpscalePixelArt
        | FsPostFilterCrtSimple
        | FsPostFilterCrtFull
        | FsPostFilterCrtArcade
        | FsPostFilterDither1bit
        | FsPostFilterGameBoy
        | FsPostFilterPc98
        | FsPostFilterGameGear
        | FsPostFilterFamicom
        | FsPostFilterMegaDrive
        | FsPostFilterMsx2Plus
        | FsPostFilterSfc
        | FsPostFilterComboFamicomCrt
        | FsPostFilterComboPc98Crt
        | FsPostFilterComboMsx2PlusCrt
        | FsPostFilterComboMegaDriveCrt
        | FsPostFilterComboSfcCrt
        | FsPostFilterSepia
        | FsPostFilterMonoNeutral
        | FsPostFilterMonoCool
        | FsPostFilterMonoWarm
        | FsPostFilterWarmTone
        | FsPostFilterCoolTone
        | FsPostFilterTealOrange
        | FsPostFilterKodakPortra
        | FsPostFilterFujiVelvia
        | FsPostFilterBleachBypass
        | FsPostFilterCrossProcess
        | FsPostFilterVintage
        | FsPostFilterFilmGrain
        | FsPostFilterVignette
        | FsPostFilterLightLeak
        | FsPostFilterSoftFocus
        | FsPostFilterHalftone
        | FsPostFilterOilPaint
        | FsPostFilterSketch
        | FsPostFilterPseudoColor4
        | FsPostFilterPseudoColorSkin
        | FsPostFilterSharpen
        | FsAdjustSlot1
        | FsAdjustSlot2
        | FsAdjustSlot3
        | FsAdjustSlot4
        | FsAdjustSlot5
        | FsAdjustSlot6
        | FsAdjustSlot7
        | FsAdjustSlot8
        | FsAdjustSlot9
        | FsAdjustSlot10
        | FsAdjustSlotDefault1
        | FsAdjustSlotDefault2
        | FsAdjustSlotDefault3
        | FsAdjustSlotDefault4
        | FsAdjustSlotDefault5
        | FsAdjustSlotDefault6
        | FsAdjustSlotDefault7
        | FsAdjustSlotDefault8
        | FsAdjustSlotDefault9
        | FsAdjustSlotDefault10
        | FsAdjustCopyGlobalDefaultToFavorite
        | FsClearAdjust
        | FsApplyErase1
        | FsApplyErase2
        | FsApplyConceal1
        | FsApplyConceal2
        | FsDeleteEraseMask
        | FsDeleteConcealMask
        | VideoPlayPause
        | VideoSeekStart
        | VideoSeekBackSmall
        | VideoSeekForwardSmall
        | VideoSeekBackMedium
        | VideoSeekForwardMedium
        | VideoSeekBackLarge
        | VideoSeekForwardLarge
        | VideoFrameStepBack
        | VideoFrameStepForward
        | VideoVolumeUp
        | VideoVolumeDown
        | VideoMute
        | VideoLoop
        | VideoMarkerPrev
        | VideoMarkerNext
        | VideoPin
        | VideoPerfOverlay
        | VideoTileMode
        | VideoSeekStripCycle
        | VideoSeekStripToggle
        | VideoSeekStripNone
        | VideoSeekStripThumbnails
        | VideoSeekStripWaveform
        | VideoScaleFilterNext
        | VideoAnime4kRemeasure
        | VideoBookmark
        | VideoCapture
        | VideoAddToActiveBook
        | VideoAddToCollectionTarget
        | VideoCompareToggle
        | VideoCompareCycle
        | VideoCompareWipe
        | VideoCompareDiff
        | VideoAdjustSlot1
        | VideoAdjustSlot2
        | VideoAdjustSlot3
        | VideoAdjustSlot4
        | VideoAdjustSlot5
        | VideoAdjustSlot6
        | VideoAdjustSlot7
        | VideoAdjustSlot8
        | VideoAdjustSlot9
        | VideoAdjustSlot10
        | VideoToggleAudioMode
        | VideoNextAudioTrack
        | EraseConfirm
        | EraseConfirmPolygon
        | EraseRedo
        | EraseUndo
        | EraseDeleteShape
        | EraseToolSelect
        | EraseToolBrush
        | EraseToolBucket
        | EraseToolLasso
        | EraseToolPolygon
        | EraseToolVLine
        | EraseToolHLine
        | EraseToolLine
        | EraseToolRect
        | EraseToolEllipse
        | ErasePaintMode
        | EraseEraseMode
        | EraseSpacePan
        | EraseScrubZoomHold
        | ErasePreviewHold
        | ConcealExit
        | ConcealConfirmPolygon
        | ConcealRedo
        | ConcealUndo
        | ConcealDeleteShape
        | ConcealPixelGrid
        | ConcealTypeCycle
        | ConcealPreset1
        | ConcealPreset2
        | ConcealPreset3
        | ConcealPreset4
        | ConcealPaintMode
        | ConcealEraseMode
        | ConcealToolSelect
        | ConcealToolBrush
        | ConcealToolBucket
        | ConcealToolLasso
        | ConcealToolPolygon
        | ConcealToolLine
        | ConcealToolVLine
        | ConcealToolHLine
        | ConcealToolRect
        | ConcealToolEllipse
        | ConcealSpacePan
        | ConcealScrubZoomHold
        | ConcealPreviewHold
        | CropForceCreateHold
        | CropSpacePan
        | CropScrubZoomHold
        | CropExecute
        | SnsSplitExecute
        | TextConfirm
        | TextRedo
        | TextUndo
        | TextSpacePan
        | TextScrubZoomHold
        | LaShowSource
        | LaShowMask
        | LaPaintAdd
        | LaPaintErase
        | LaToolBrush
        | LaToolBucket
        | LaToolEdgeBrush
        | LaToolGapFill
        | LaToolLasso
        | LaToolPolygon
        | LaToolSelect
        | LaToolLine
        | LaToolVLine
        | LaToolHLine
        | LaToolRect
        | LaToolEllipse
        | LaConfirmPolygon
        | LaDeleteShape
        | LaSpacePan
        | LaScrubZoomHold => Entry::NoLocationChange,
    }
}

fn menu_entry(action: MenuCommandId) -> MenuEntry {
    use MenuCommandId::*;
    match action {
        FileOpenFolder => MenuEntry::FolderDialog,
        FileReadingHistory => MenuEntry::ReadingHistory,
        BooksOpenRoot => MenuEntry::BooksRoot,
        BooksOpenActiveBook => MenuEntry::SavedBook,
        CollectionsOpenTarget => MenuEntry::SavedCollection,
        BooksManage => MenuEntry::BookManager,
        CollectionsManage => MenuEntry::CollectionManager,
        FileLocalSearch
        | GlobalEverythingSearch
        | FavoritesFavSearch
        | FavoritesMetadataSearch
        | TagsTagView => MenuEntry::SearchSurface,
        FileReload => MenuEntry::Refresh,
        SmartFoldersNew | SmartFoldersAddCurrent | SmartFoldersManage => MenuEntry::LogicalView,
        FileOpenCaptureFolder | FileOpenRecycleBin | HelpOpenManual | HelpOpenLogs => {
            MenuEntry::External
        }
        FileMetadataExport
        | FileMetadataImport
        | FileQuit
        | FavoritesAddCurrentFolder
        | FavoritesEdit
        | BooksAddSelectionToActiveBook
        | BooksAddClipboardImage
        | BooksReorderCurrentBook
        | CollectionsAddSelectionToTarget
        | CollectionsImportCurrent
        | CollectionsExportCurrent
        | CollectionsExportAll
        | CollectionsSetOrderCurrent
        | CollectionsRelinkCurrent
        | CollectionsReorderCurrent
        | TagsManagePinned
        | ConvertToZip
        | ToolsCreateAiCaptions
        | ToolsShowCaptionProgress
        | VideoRegisterUpscale
        | VideoDeleteUpscale
        | VideoShowUpscaleTasks
        | SettingsThumbnailCache
        | SettingsAlwaysOnTop
        | SettingsArchiveCache
        | SettingsEpubCache
        | SettingsThumbnailQuality
        | SettingsStats
        | SettingsResetRotation
        | SettingsRestoreSettings
        | SettingsOperationCustomize
        | SettingsPreferences
        | SettingsRemoteConnection
        | SettingsAgentIntegration
        | HelpShowWhatsNew
        | HelpAbout => MenuEntry::NoLocationChange,
    }
}

#[test]
fn search_nav_inventory_key_and_menu_registries_have_explicit_policies() {
    use std::collections::HashSet;
    let actions = KeyAction::all();
    assert_eq!(
        actions.iter().copied().collect::<HashSet<_>>().len(),
        actions.len()
    );
    let menus = MenuCommandId::ALL;
    assert_eq!(
        menus.iter().copied().collect::<HashSet<_>>().len(),
        menus.len()
    );
    // Both matches are exhaustive. The full registry, not a separately curated
    // navigation list, is the input to the behavioral tests below.
    for &action in actions {
        let _ = key_entry(action);
    }
    for &id in menus {
        let _ = menu_entry(id);
        assert!(crate::keymap::menu_command_spec(id).is_some(), "{id:?}");
    }
}

#[test]
fn search_nav_inventory_metadata_all_independent_key_inputs() {
    key_inputs_for_search(0);
}

#[test]
fn search_nav_inventory_everything_all_independent_key_inputs() {
    key_inputs_for_search(1);
}

#[test]
fn search_nav_inventory_container_all_independent_key_inputs() {
    key_inputs_for_search(2);
}

#[test]
fn search_nav_inventory_tag_all_independent_key_inputs() {
    key_inputs_for_search(3);
}

fn key_inputs_for_search(kind: usize) {
    for &action in KeyAction::all() {
        match key_entry(action) {
            Entry::SavedGroup => {
                super::search_nav_book_tests::assert_saved_group_search_action(action, kind);
            }
            Entry::BookManager => {
                super::search_nav_book_tests::assert_saved_group_search_entry(
                    KeyAction::GridOpenActiveBook,
                    kind,
                    |app, ctx| {
                        assert!(app.apply_saved_group_key_action(ctx, action).is_none());
                        assert!(app.show_book_manager);
                        click_manager_open(app, false);
                        None
                    },
                );
            }
            Entry::CollectionManager => {
                super::search_nav_book_tests::assert_saved_group_search_entry(
                    KeyAction::GridOpenCollectionTarget,
                    kind,
                    |app, ctx| {
                        assert!(app.apply_saved_group_key_action(ctx, action).is_none());
                        assert!(app.collection_manager_open());
                        click_manager_open(app, true);
                        None
                    },
                );
            }
            Entry::FolderDialog => folder_dialog_case(kind, None),
            Entry::PhysicalParent => physical_parent_case(kind),
            Entry::Favorite | Entry::Location => location_key_case(action, kind),
            // These operations belong to the selected result/current reading
            // session, refuse search history, refresh the same source, or do not
            // navigate internally. Turning any of them into an independent
            // destination requires changing the explicit policy match above.
            Entry::ResultNavigation
            | Entry::History
            | Entry::Refresh
            | Entry::SearchSurface
            | Entry::External
            | Entry::NoLocationChange => {}
        }
    }
}

#[test]
fn search_nav_inventory_all_navigation_menus_use_their_real_handlers() {
    for kind in 0..4 {
        for &id in MenuCommandId::ALL {
            match menu_entry(id) {
                MenuEntry::SavedBook | MenuEntry::BookManager => {
                    super::search_nav_book_tests::assert_saved_group_search_entry(
                        KeyAction::GridOpenActiveBook,
                        kind,
                        |app, _| {
                            assert!(click_menu(app, id).is_none());
                            if menu_entry(id) == MenuEntry::BookManager {
                                assert!(app.show_book_manager);
                                click_manager_open(app, false);
                            }
                            None
                        },
                    );
                }
                MenuEntry::SavedCollection | MenuEntry::CollectionManager => {
                    super::search_nav_book_tests::assert_saved_group_search_entry(
                        KeyAction::GridOpenCollectionTarget,
                        kind,
                        |app, _| {
                            assert!(click_menu(app, id).is_none());
                            if menu_entry(id) == MenuEntry::CollectionManager {
                                assert!(app.collection_manager_open());
                                click_manager_open(app, true);
                            }
                            None
                        },
                    );
                }
                MenuEntry::FolderDialog => folder_dialog_case(kind, Some(id)),
                MenuEntry::ReadingHistory => {
                    let mut app = fixture(kind);
                    let history = app.folder_nav_history_snapshot();
                    // The file-menu entrance predates LocationJump and keeps
                    // its released Restore contract while retiring search.
                    assert!(click_menu(&mut app, id).is_none());
                    settle(&mut app, &egui::Context::default());
                    assert!(!search_active(&app), "{id:?}: install must retire search");
                    assert!(app.items_are_reading_history_view);
                    assert!(matches!(
                        app.top_level_grid_view.surface(),
                        top_level_grid_view::TopLevelGridSurface::ReadingHistory(_)
                    ));
                    assert_eq!(
                        app.folder_nav_history_snapshot(),
                        history,
                        "{id:?}: the existing menu Restore entrance must not record history"
                    );
                }
                MenuEntry::BooksRoot => {
                    let mut app = fixture(kind);
                    let origin = app.tmp.path().join("pre-search");
                    app.settings.book_root = Some(app.tmp.path().join("inventory-shelf"));
                    let ctx = egui::Context::default();
                    let candidate = click_menu(&mut app, id);
                    let mut frame = empty_navigation();
                    frame.fav_nav = candidate;
                    app.post_render_main_navigation(&ctx, frame);
                    settle(&mut app, &ctx);
                    assert_departed_and_back(&mut app, &ctx, &origin, &format!("{id:?}"));
                }
                MenuEntry::SearchSurface
                | MenuEntry::Refresh
                | MenuEntry::LogicalView
                | MenuEntry::External
                | MenuEntry::NoLocationChange => {}
            }
        }
    }
}

fn fixture(kind: usize) -> super::tests::phase_c_support::AppTestEnv {
    let mut app = App::navigation_search_fixture_for_test(kind);
    let origin = app.tmp.path().join("pre-search");
    std::fs::create_dir_all(&origin).unwrap();
    app.settings.auto_fullscreen_zip_pdf = false;
    app.settings.auto_fullscreen_image_folders = false;
    app.fs_feedback_toast = None;
    app
}

fn location_key_case(action: KeyAction, kind: usize) {
    let mut app = fixture(kind);
    let origin = app.tmp.path().join("pre-search");
    let shelf = app.tmp.path().join("inventory-shelf");
    app.settings.book_root = Some(shelf);
    if key_entry(action) == Entry::Favorite {
        let entries = (1..=20)
            .map(|slot| {
                let path = app.tmp.path().join(format!("Favorite{slot:02}"));
                std::fs::create_dir(&path).unwrap();
                crate::settings::FavoriteEntry::new(format!("Favorite{slot:02}"), path)
            })
            .collect();
        app.settings.favorites = entries;
    }
    let before = source_state(&app);
    let ctx = egui::Context::default();
    let candidate = app.apply_location_navigation_key_action(&ctx, action, "entry-inventory");
    assert_eq!(
        source_state(&app),
        before,
        "producer changed accepted source: {action:?}"
    );
    if let Some(candidate) = candidate {
        match (&candidate, key_entry(action)) {
            (AddressBarNav::Favorite(path), Entry::Favorite) => {
                let slot = action.favorite_slot_number().unwrap_or_else(|| {
                    if action == KeyAction::GridFavoritePrev {
                        20
                    } else {
                        1
                    }
                });
                assert_eq!(path, &app.settings.favorites[slot - 1].path);
            }
            (AddressBarNav::LocationJump(_), Entry::Location) => {}
            _ => panic!("{action:?} returned an incompatible transport: {candidate:?}"),
        }
        let mut frame = empty_navigation();
        frame.keyboard_nav = Some(candidate);
        app.post_render_main_navigation(&ctx, frame);
        settle(&mut app, &ctx);
        if search_active(&app) {
            // An unavailable/inaccessible host drive may fail. A failed open
            // must retain the complete accepted source, never a physical child.
            assert_eq!(
                source_state(&app),
                before,
                "failed {action:?} replaced search"
            );
            assert!(
                app.fs_feedback_toast.is_some(),
                "{action:?}: failure must notify"
            );
        } else {
            assert_departed_and_back(&mut app, &ctx, &origin, &format!("{action:?}"));
        }
    } else {
        // Drive/current-root/known-folder resolution can refuse a missing host
        // location before producing a request. That refusal is an explicit
        // result, not permission to omit the action from this inventory.
        assert!(
            search_active(&app),
            "{action:?}: refused producer closed search"
        );
        assert_eq!(source_state(&app), before);
        assert!(
            app.fs_feedback_toast.is_some(),
            "{action:?}: refused producer must notify"
        );
    }
}

fn physical_parent_case(kind: usize) {
    let mut app = super::tests::phase_c_support::setup_app();
    app.active_quick_folder_slot = None;
    app.settings.auto_fullscreen_zip_pdf = false;
    app.settings.auto_fullscreen_image_folders = false;
    app.settings.detached_viewer_open_images_in_window = false;
    let origin = app.tmp.path().join("pre-search");
    let parent = app.tmp.path().join("inventory-physical-parent");
    let child = parent.join("child");
    std::fs::create_dir_all(&origin).unwrap();
    std::fs::create_dir_all(&child).unwrap();
    app.load_folder(origin.clone());
    let ctx = egui::Context::default();
    admit_search_result_child(&mut app, &ctx, kind, &child);
    assert!(app.grid_physical_parent_action().available());
    let candidate = app.handle_grid_physical_parent_action();
    let mut frame = empty_navigation();
    frame.keyboard_nav = candidate;
    app.post_render_main_navigation(&ctx, frame);
    settle(&mut app, &ctx);
    assert_eq!(app.current_folder.as_ref(), Some(&parent));
    assert_departed_and_back(&mut app, &ctx, &origin, "GridPhysicalParentFolder");
}

/// Start from an already adopted ordinary origin (including its facet route),
/// publish a genuine search result, and enter that row through the grid handler.
/// Other regressions reuse this without manually manufacturing a PhysicalChild.
pub(super) fn admit_search_result_child(
    app: &mut App,
    ctx: &egui::Context,
    kind: usize,
    child: &Path,
) {
    assert!(
        !search_active(app),
        "result admission starts at the real origin"
    );
    match kind {
        0 => {
            app.open_global_search();
            app.global_search.aggregate_auto = false;
            // The prepare fixture submits "test" as last_executed. Keep the
            // draft equal so real Ready adoption can prove an applied query.
            app.global_search.metadata_draft = "test".into();
            // Opening a retained editor can mark its old draft done. This is
            // a fresh producer: its initial view wish must also see done=false.
            app.global_search.done = false;
            app.begin_search_page_edit_prepare_for_test(ctx);
            let (stream, rx) = crossbeam_channel::unbounded();
            app.global_search.pending = Some(crate::global_search_ui::SearchProducer::Metadata(
                crate::indexer_manager::SearchHandle {
                    cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    rx,
                },
            ));
            stream
                .send(crate::global_search::SearchStreamEvent::Batch {
                    hits: vec![crate::global_search::GlobalHit {
                        path: child.to_string_lossy().into_owned(),
                        kind: crate::fts_index::IndexKind::Folder,
                        score: 1.0,
                        stars: 0,
                        mtime: 0,
                        file_size: Some(0),
                    }],
                    scanned_candidates: 1,
                    valid_hits: 1,
                    visibility_notice: None,
                })
                .unwrap();
            stream
                .send(crate::global_search::SearchStreamEvent::Done {
                    truncated: false,
                    reason: crate::global_search::DoneReason::Complete,
                })
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                app.poll_global_search_events(ctx);
                if app.global_search.done
                    && app
                        .items
                        .iter()
                        .any(|item| matches!(item, GridItem::Folder(path) if path == child))
                    && matches!(
                        app.global_search.accepted,
                        crate::global_search_ui::SearchAccepted::Metadata { applied: Some(_) }
                    )
                {
                    break;
                }
                if Instant::now() >= deadline {
                    let accepted = match &app.global_search.accepted {
                        crate::global_search_ui::SearchAccepted::Metadata { applied } => {
                            format!("Metadata({applied:?})")
                        }
                        crate::global_search_ui::SearchAccepted::Everything { .. } => {
                            "Everything".into()
                        }
                    };
                    let metrics = app.search_prepare_metrics_for_test();
                    panic!(
                        "metadata folder result did not adopt: child={child:?} active={} done={} accepted={} view={:?} hits_len={} hits={:?} items_len={} items={:?} source={:?} current_folder={:?} draft={:?} last_executed={:?} rejected={:?} items_are_global_search_view={} run_sequence={} rated_batches={} stale_rejections={}",
                        app.global_search.active,
                        app.global_search.done,
                        accepted,
                        app.global_search.view(),
                        app.global_search.all_hits.len(),
                        app.global_search
                            .all_hits
                            .iter()
                            .map(|hit| (&hit.path, &hit.kind))
                            .collect::<Vec<_>>(),
                        app.items.len(),
                        app.items.iter().map(GridItem::perf_key).collect::<Vec<_>>(),
                        app.global_search.source,
                        app.current_folder,
                        app.global_search.metadata_draft,
                        app.global_search.last_executed,
                        app.global_search.reject_message,
                        app.items_are_global_search_view,
                        app.global_search.run_sequence,
                        metrics.rated_batches,
                        metrics.stale_rejections,
                    );
                }
                std::thread::yield_now();
            }
        }
        1 => crate::global_search_ui::adopt_everything_for_test(
            app,
            ctx,
            "inventory child",
            crate::global_search_ui::everything_dataset_for_test(vec![GridItem::Folder(
                child.to_path_buf(),
            )]),
        ),
        2 => {
            app.open_favsearch();
            app.favsearch.query = "inventory child".into();
            app.favsearch.last_executed = app.favsearch.query.clone();
            app.apply_favsearch_results(vec![crate::search_index_db::IndexEntry {
                path: child.to_path_buf(),
                display_name: "child".into(),
                kind: crate::search_index_db::IndexKind::Folder,
                mtime: 0,
            }]);
        }
        3 => {
            app.tag_view.query = "inventory child".into();
            app.open_tag_view();
            if let Some(pending) = app.tag_view_pending.take() {
                pending.cancel();
            }
            app.apply_tag_view_result(crate::tag_view::TagViewResult {
                query: app.tag_view.query.clone(),
                kind_filter: app.tag_view.kind_filter,
                summaries: Vec::new(),
                entries: vec![crate::tag_view::TagViewEntry {
                    path: child.to_path_buf(),
                    kind: crate::tag_view::TagViewItemKind::Folder,
                    mtime: 0,
                    file_size: 0,
                    applied_at_ms: None,
                }],
                truncated: false,
                video_thumb_overrides: Default::default(),
            });
        }
        _ => panic!("unknown search fixture {kind}"),
    }
    app.finish_page_identity_preparation_for_test();
    assert!(search_active(app));
    let index = app
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Folder(path) if path == child))
        .expect("the adopted search dataset must contain the real folder row");
    app.selected = Some(index);
    assert!(app.visible_indices.contains(&index));
    let candidate = app
        .handle_gamepad_grid_accept(ctx)
        .expect("the actual result-row handler must produce physical navigation");
    let mut frame = empty_navigation();
    frame.gamepad_nav = Some(candidate);
    app.post_render_main_navigation(ctx, frame);
    settle(app, ctx);
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.current_folder.as_deref(), Some(child));
    assert!(
        search_active(app),
        "descending a result keeps its search owner"
    );
    match kind {
        0 => assert!(app.global_search.drill.is_some()),
        1 => assert!(matches!(
            &app.global_search.accepted,
            crate::global_search_ui::SearchAccepted::Everything {
                position: crate::global_search_ui::EverythingPosition::PhysicalChild { logical_source, .. },
                ..
            } if logical_source == child
        )),
        2 => assert_eq!(
            app.favsearch.nav_stack.last().map(PathBuf::as_path),
            Some(child)
        ),
        3 => assert_eq!(
            app.tag_view.nav_stack.last().map(PathBuf::as_path),
            Some(child)
        ),
        _ => unreachable!(),
    }
}

fn folder_dialog_case(kind: usize, menu: Option<MenuCommandId>) {
    let _input = crate::key_input::lock_test_input();
    let mut app = fixture(kind);
    let origin = app.tmp.path().join("pre-search");
    let target = app.tmp.path().join("inventory-folder-dialog");
    std::fs::create_dir(&target).unwrap();
    let ctx = egui::Context::default();
    if let Some(id) = menu {
        assert!(click_menu(&mut app, id).is_none());
    } else {
        app.keymap = crate::keymap::Keymap::from_ini_str("[Global]\nGlobalOpenFolder = F14\n");
        crate::ime_focus::install_ime_input_policy(&ctx);
        ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::F14,
                    physical_key: Some(egui::Key::F14),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| {
                app.update_ime_state(ui.ctx());
                let _ = app.shortcuts_blocked_by_text_input(ui.ctx());
                app.input_main_grid_shortcuts(ui.ctx(), false);
            },
        )
        .drop_without_applying_deltas();
    }
    assert!(
        app.show_open_folder_dialog,
        "{menu:?}: actual command must show the folder dialog"
    );
    app.open_folder_input = target.to_string_lossy().into_owned();
    let result = RefCell::new(None);
    {
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(1100.0, 500.0))
            .build_ui(|ui| {
                app.update_ime_state(ui.ctx());
                if let Some(path) = app.show_open_folder_dialog_window(ui.ctx()) {
                    assert!(
                        result.borrow_mut().replace(path).is_none(),
                        "one confirmation"
                    );
                }
            });
        harness.run();
        click_label(&mut harness, "  開く  ");
    }
    let candidate = result
        .into_inner()
        .expect("real dialog confirms its physical destination");
    assert_eq!(candidate, target);
    let mut frame = empty_navigation();
    frame.open_folder_nav = Some(candidate);
    app.post_render_main_navigation(&ctx, frame);
    settle(&mut app, &ctx);
    assert_eq!(app.current_folder.as_ref(), Some(&target));
    assert_departed_and_back(&mut app, &ctx, &origin, "FileOpenFolder/GlobalOpenFolder");
}

fn click_menu(app: &mut App, id: MenuCommandId) -> Option<LocationJumpTarget> {
    let spec = crate::keymap::menu_command_spec(id).unwrap();
    let label = app.keymap.menu_command_label(id);
    let candidate = RefCell::new(None);
    {
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(1600.0, 700.0))
            .build_ui(|ui| {
                app.note_main_chrome_pointer_block_transition(ui.ctx());
                if let (Some(target), _) = app.render_menubar(ui) {
                    assert!(
                        candidate.borrow_mut().replace(target).is_none(),
                        "{id:?}: one click"
                    );
                }
            });
        harness.run();
        click_label(&mut harness, spec.parent.label());
        click_label(&mut harness, &label);
    }
    candidate.into_inner()
}

fn click_manager_open(app: &mut App, collection: bool) {
    let mut harness = crate::test_ui::builder()
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .with_size(egui::vec2(1600.0, 900.0))
        .build_ui(|ui| {
            if collection {
                app.show_collection_manager(ui.ctx());
            } else {
                app.draw_book_manager(ui.ctx());
            }
        });
    harness.run();
    // Both managers have row buttons and a distinct top button for the active
    // append target. Select that real top control by geometry, not list order.
    let position = harness
        .get_all_by_label("開く")
        .map(|node| node.rect().center())
        .min_by(|a, b| a.y.total_cmp(&b.y))
        .expect("manager must offer its active target");
    click_at(&mut harness, position);
}

fn click_label(harness: &mut egui_kittest::Harness<'_, ()>, label: &str) {
    let position = harness.get_by_label(label).rect().center();
    click_at(harness, position);
}

fn click_at(harness: &mut egui_kittest::Harness<'_, ()>, position: egui::Pos2) {
    harness.hover_at(position);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        harness.step();
    }
    harness.run();
}

fn settle(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_rating_view();
        app.poll_sidecar_restore(ctx);
        app.post_render_main_navigation(ctx, empty_navigation());
        app.settle_open_path_classification_for_test();
        app.poll_history_navigation_transition_for_test(ctx);
        if app.folder_pane_open_pending.is_none()
            && app.rating_view_pending.is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && !app.sidecar_restore_active()
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "inventory destination did not settle"
        );
        std::thread::yield_now();
    }
}

fn assert_departed_and_back(app: &mut App, ctx: &egui::Context, origin: &Path, entry: &str) {
    assert!(
        !search_active(app),
        "{entry}: accepted independent destination retained search"
    );
    assert_eq!(
        app.folder_history_back_entry(),
        Some(&FolderNavHistoryTarget::Path(origin.to_path_buf()).into()),
        "{entry}: Back must refer to the exact pre-search location",
    );
    let nav = app
        .history_key_navigation(FolderHistoryDirection::Back)
        .unwrap_or_else(|| panic!("{entry}: real Back handler refused"));
    let mut frame = empty_navigation();
    frame.keyboard_nav = Some(nav);
    app.post_render_main_navigation(ctx, frame);
    settle(app, ctx);
    assert_eq!(
        app.current_folder.as_deref(),
        Some(origin),
        "{entry}: actual Back destination"
    );
    assert!(!search_active(app));
}

fn search_active(app: &App) -> bool {
    app.global_search.active || app.favsearch.active || app.tag_view.active
}

fn source_state(app: &App) -> String {
    format!(
        "{:?}",
        (
            (
                &app.current_folder,
                &app.address,
                app.selected,
                app.items_generation
            ),
            (
                app.items
                    .iter()
                    .map(GridItem::drag_source_path)
                    .collect::<Vec<_>>(),
                &app.checked,
                app.folder_nav_history_snapshot(),
            ),
            (
                app.top_level_grid_view.surface(),
                app.top_level_grid_view.return_to()
            ),
        )
    )
}

fn empty_navigation() -> update_phases::MainFrameNavigation {
    update_phases::MainFrameNavigation {
        fav_nav: None,
        toolbar_fav_nav: None,
        fullscreen_close_nav: None,
        keyboard_nav: None,
        keyboard_nav_from_fullscreen_close: false,
        gamepad_nav: None,
        address_nav: None,
        open_folder_nav: None,
        context_nav: None,
        folder_pane_nav: None,
        grid_nav: None,
    }
}
