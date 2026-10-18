use crate::{
    app::{Message, State},
    icons,
    services::jobs::{JobKind, JobView},
    theme,
};
use ferrolibri_core::jobs::{JobOutput, JobState, Stage};
use iced::{
    Element, Fill,
    widget::{Space, button, column, container, progress_bar, row, scrollable, text},
};

pub use crate::services::jobs::HISTORY_LIMIT;
pub fn unfinished(job: &JobView) -> bool {
    matches!(job.state, JobState::Waiting | JobState::Running)
}
/// Housekeeping such as reading a book's metadata or probing for converters
/// runs as a job too, but is not something the user asked for: keep it out of the list.
pub fn listed(job: &JobView) -> bool {
    !matches!(job.kind, JobKind::Metadata | JobKind::Backends)
}
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}
/// `Downloading · 1.2 MB of 3.4 MB`, or `Downloading · 1.2 MB received` when the size is not known.
fn downloading(job: &JobView) -> String {
    match (job.received, job.progress) {
        (None, _) => "Getting link…".into(),
        (Some(received), Some((_, total))) => {
            format!(
                "Downloading · {} of {}",
                megabytes(received),
                megabytes(total)
            )
        }
        (Some(received), None) => format!("Downloading · {} received", megabytes(received)),
    }
}
pub fn percentage(job: &JobView) -> Option<f32> {
    let copying = job.kind == JobKind::Send && job.stage == Some(Stage::Copying);
    let downloading = job.kind == JobKind::Download && job.stage == Some(Stage::Downloading);
    if !copying && !downloading {
        return None;
    }
    job.progress.map(|(completed, total)| {
        if total == 0 {
            0.0
        } else {
            (completed.min(total) as f64 / total as f64 * 100.0) as f32
        }
    })
}
pub fn status(job: &JobView) -> String {
    match job.state {
        JobState::Waiting => "Waiting".into(),
        JobState::Succeeded => "Done".into(),
        JobState::Cancelled => "Cancelled".into(),
        JobState::Failed => job.error.clone().unwrap_or_else(|| "Job failed".into()),
        JobState::Running => match job.stage {
            Some(Stage::Scanning) => "Scanning library…".into(),
            Some(Stage::Preparing) => "Preparing…".into(),
            Some(Stage::Converting) => "Converting…".into(),
            Some(Stage::Copying) => percentage(job)
                .map(|p| format!("Copying · {p:.0}%"))
                .unwrap_or_else(|| "Copying…".into()),
            Some(Stage::Downloading) => downloading(job),
            Some(Stage::Adding) => "Adding to library…".into(),
            None => "Starting…".into(),
        },
    }
}
fn result_text(state: &State, job: &JobView) -> String {
    if job.state != JobState::Succeeded {
        return status(job);
    }
    if let Some(message) = state.job_messages.get(&job.id) {
        return message.clone();
    }
    match state.results.get(&job.id).map(|r| r.as_ref()) {
        Some(Ok(JobOutput::Sent(path))) => format!("Sent · {}", path.display()),
        Some(Ok(JobOutput::Conversion(result))) => {
            format!("Converted · {}", result.output_path.display())
        }
        Some(Ok(JobOutput::Added(book))) => match book.outcome {
            ferrolibri_core::calibre::AddOutcome::Added { .. } => {
                format!("Added to your library · {}", book.title)
            }
            ferrolibri_core::calibre::AddOutcome::AlreadyExists => {
                format!("Already in your library · not added again · {}", book.title)
            }
        },
        _ if job.kind == JobKind::Scan => "Scanned".into(),
        _ => status(job),
    }
}
/// The bar of a download whose size is not known: a piece that sweeps along the track.
fn moving_bar<'a>(tick: u32) -> Element<'a, Message> {
    const WIDTH: f32 = 80.0;
    const PIECE: f32 = 32.0;
    let offset = (tick % 20) as f32 / 20.0 * (WIDTH - PIECE);
    container(row![
        Space::new().width(offset),
        container(Space::new())
            .width(PIECE)
            .height(3)
            .style(|_| container::Style {
                background: Some(theme::DARK.accent.into()),
                ..Default::default()
            }),
    ])
    .width(WIDTH)
    .height(3)
    .clip(true)
    .style(|_| container::Style {
        background: Some(theme::DARK.hover.into()),
        ..Default::default()
    })
    .into()
}
/// The cancel button of a job. Once a downloaded book is being handed to Calibre it cannot be
/// stopped (the design's assumption: a `calibredb` that is killed half way may leave the library
/// half written), so the button stays but is inactive and says why.
fn cancel_button<'a>(state: &State, job: &JobView) -> Element<'a, Message> {
    let adding = job.kind == JobKind::Download && job.stage == Some(Stage::Adding);
    let size = theme::font(state, 12.0);
    let control = button(text("Cancel").size(size))
        .style(theme::outline)
        .on_press_maybe((!adding).then_some(Message::Cancel(job.id)));
    if adding {
        super::tip(
            control,
            text("Adding to the library cannot be cancelled").size(size),
            super::Position::Left,
        )
    } else {
        control.into()
    }
}
fn job_row<'a>(state: &'a State, job: &'a JobView) -> Element<'a, Message> {
    let download = job.kind == JobKind::Download;
    let mut status = result_text(state, job);
    if download && let Some(plugin) = state.downloads.plugin_of(job.id) {
        status = format!("{plugin} · {status}");
    }
    let title = text(&job.title).size(theme::font(state, 13.0));
    let title: Element<'a, Message> = if download {
        row![icons::view(icons::DOWNLOAD), title]
            .spacing(6)
            .align_y(iced::Alignment::Center)
            .into()
    } else {
        title.into()
    };
    let mut contents = row![
        column![
            title,
            text(status)
                .size(theme::font(state, 12.0))
                .color(if job.state == JobState::Failed {
                    theme::DARK.error
                } else {
                    theme::DARK.text_secondary
                })
        ]
        .spacing(4)
        .width(Fill)
    ]
    .spacing(12)
    .align_y(iced::Alignment::Center);
    if unfinished(job) {
        if let Some(p) = percentage(job) {
            contents = contents.push(progress_bar(0.0..=100.0, p).length(80).girth(3));
        } else if download
            && job.state == JobState::Running
            && job.stage == Some(Stage::Downloading)
            && job.received.is_some()
        {
            contents = contents.push(moving_bar(state.downloads.spin()));
        }
        contents = contents.push(cancel_button(state, job));
    }
    container(contents).padding([8, 12]).width(Fill).into()
}
pub fn view(state: &State) -> Element<'_, Message> {
    let header = row![
        text("Jobs").size(theme::font(state, 16.0)),
        Space::new().width(Fill),
        button(text("Clear finished").size(theme::font(state, 12.0)))
            .style(theme::quiet)
            .on_press_maybe(
                state
                    .jobs
                    .values()
                    .any(|j| listed(j) && !unfinished(j))
                    .then_some(Message::ClearFinished)
            )
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);
    let mut body = column![].spacing(4);
    if !state.jobs.values().any(listed) {
        body = body.push(
            container(
                text("No jobs yet.\nSend or convert a book to start one.")
                    .size(theme::font(state, 13.0))
                    .color(theme::DARK.text_muted),
            )
            .padding(24)
            .center_x(Fill),
        );
    } else {
        for (label, group) in [
            ("Running", JobState::Running),
            ("Up next", JobState::Waiting),
        ] {
            let jobs: Vec<_> = state
                .jobs
                .values()
                .filter(|j| listed(j) && j.state == group)
                .collect();
            if !jobs.is_empty() {
                body = body.push(
                    container(
                        text(label)
                            .size(theme::font(state, 11.0))
                            .color(theme::DARK.text_muted),
                    )
                    .padding([10, 12]),
                );
            }
            for job in jobs {
                body = body.push(job_row(state, job));
            }
        }
        let done: Vec<_> = state
            .finished_jobs
            .iter()
            .rev()
            .filter_map(|id| state.jobs.get(id))
            .filter(|job| listed(job))
            .collect();
        if !done.is_empty() {
            body = body.push(
                container(
                    text("Done · last 20")
                        .size(theme::font(state, 11.0))
                        .color(theme::DARK.text_muted),
                )
                .padding([10, 12]),
            );
        }
        for job in done {
            body = body.push(job_row(state, job));
        }
    }
    column![
        container(header)
            .padding(super::book_list::header_padding())
            .height(36.0 + theme::HEADER_TOP),
        scrollable(body)
            .height(Fill)
            .direction(theme::vbar())
            .style(theme::scroll(state))
    ]
    .height(Fill)
    .into()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_user_visible_jobs_are_listed() {
        let mut job = JobView {
            id: crate::services::jobs::JobId(1),
            title: "Book".into(),
            kind: JobKind::Metadata,
            state: JobState::Succeeded,
            stage: None,
            progress: None,
            received: None,
            error: None,
        };
        assert!(!listed(&job));
        job.kind = JobKind::Backends;
        assert!(!listed(&job));
        for kind in [
            JobKind::Scan,
            JobKind::Convert,
            JobKind::Send,
            JobKind::Download,
        ] {
            job.kind = kind;
            assert!(listed(&job));
        }
    }
    #[test]
    fn progress_only_when_copying_and_bytes_are_clamped() {
        let mut job = JobView {
            id: crate::services::jobs::JobId(1),
            title: "Book".into(),
            kind: JobKind::Send,
            state: JobState::Running,
            stage: Some(Stage::Preparing),
            progress: Some((42, 100)),
            received: None,
            error: None,
        };
        assert_eq!(percentage(&job), None);
        assert_eq!(status(&job), "Preparing…");
        job.stage = Some(Stage::Copying);
        assert_eq!(percentage(&job), Some(42.0));
        assert_eq!(status(&job), "Copying · 42%");
        job.progress = Some((u64::MAX, u64::MAX));
        assert_eq!(percentage(&job), Some(100.0));
        job.progress = Some((100, 1));
        assert_eq!(percentage(&job), Some(100.0));
        job.progress = Some((0, 0));
        assert_eq!(percentage(&job), Some(0.0));
        job.kind = JobKind::Convert;
        assert_eq!(percentage(&job), None);
        job.state = JobState::Cancelled;
        assert_eq!(status(&job), "Cancelled");
    }
    #[test]
    fn a_download_shows_bytes_of_the_total_when_the_size_is_known_and_bytes_received_when_it_is_not()
     {
        let mut job = JobView {
            id: crate::services::jobs::JobId(1),
            title: "Download".into(),
            kind: JobKind::Download,
            state: JobState::Running,
            stage: Some(Stage::Downloading),
            progress: None,
            received: None,
            error: None,
        };
        assert_eq!(status(&job), "Getting link…");
        job.received = Some(2_500_000);
        assert_eq!(status(&job), "Downloading · 2.5 MB received");
        job.progress = Some((1_200_000, 3_400_000));
        assert_eq!(status(&job), "Downloading · 2.5 MB of 3.4 MB");
        job.stage = Some(Stage::Adding);
        assert_eq!(percentage(&job), None);
        assert_eq!(status(&job), "Adding to library…");
    }
}
