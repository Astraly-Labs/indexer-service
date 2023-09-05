use axum::extract::State;
use axum::Json;

use crate::domain::models::{Post, PostError};
use crate::handlers::posts::{ListPostsResponse, PostResponse};
use crate::infra::repositories::post_repository;
use crate::AppState;

pub async fn list_posts(
    State(state): State<AppState>,
) -> Result<Json<ListPostsResponse>, PostError> {
    let posts = post_repository::get_all(&state.pool)
        .await
        .map_err(|_| PostError::InternalServerError)?;

    Ok(Json(adapt_posts_to_list_posts_response(posts)))
}

fn adapt_post_to_post_response(post: Post) -> PostResponse {
    PostResponse {
        id: post.id,
        title: post.title,
        body: post.body,
        published: post.published,
    }
}

fn adapt_posts_to_list_posts_response(posts: Vec<Post>) -> ListPostsResponse {
    let posts_response: Vec<PostResponse> =
        posts.into_iter().map(adapt_post_to_post_response).collect();

    ListPostsResponse {
        posts: posts_response,
    }
}
