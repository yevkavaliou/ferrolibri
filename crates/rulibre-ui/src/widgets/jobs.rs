use crate::{
    app::{Message, State},
    services::jobs::{JobKind, JobView},
    theme,
};
use iced::{
    Element, Fill,
    widget::{Space, button, column, container, progress_bar, row, scrollable, text},
};
use rulibre_core::jobs::{JobOutput, JobState, Stage};

pub use crate::services::jobs::HISTORY_LIMIT;
pub fn unfinished(job: &JobView) -> bool {
    matches!(job.state, JobState::Waiting | JobState::Running)
}
pub fn percentage(job: &JobView) -> Option<f32> {
    if job.kind != JobKind::Send || job.stage != Some(Stage::Copying) {
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
        _ if job.kind == JobKind::Scan => "Scanned".into(),
        _ => status(job),
    }
}
fn job_row<'a>(state: &'a State, job: &'a JobView) -> Element<'a, Message> {
    let mut contents = row![
        column![
            text(&job.title).size(theme::font(state, 13.0)),
            text(result_text(state, job))
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
        }
        contents = contents.push(
            button(text("Cancel").size(theme::font(state, 12.0)))
                .style(theme::outline)
                .on_press(Message::Cancel(job.id)),
        );
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
                    .any(|j| !unfinished(j))
                    .then_some(Message::ClearFinished)
            )
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);
    let mut body = column![].spacing(4);
    if state.jobs.is_empty() {
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
            let jobs: Vec<_> = state.jobs.values().filter(|j| j.state == group).collect();
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
        container(header).padding([2, 12]).height(36),
        scrollable(body).height(Fill).style(theme::scroll)
    ]
    .height(Fill)
    .into()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_only_when_copying_and_bytes_are_clamped() {
        let mut job = JobView {
            id: crate::services::jobs::JobId(1),
            title: "Book".into(),
            kind: JobKind::Send,
            state: JobState::Running,
            stage: Some(Stage::Preparing),
            progress: Some((42, 100)),
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
}
